//! Bounded `EggPool` summary client and pane refresh worker.

use std::env;
use std::ffi::OsString;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eggfetch_core::{AuthScheme, Error as EggfetchError, NetworkFailureKind, RequestFailure};
use serde::Deserialize;
use url::Url;

use crate::clock::{Clock, RealClock};
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
    #[allow(dead_code)] // Distinguishes cancellation from transport failures.
    Cancelled,
    /// The configured endpoint cannot be represented as a valid request URL.
    InvalidEndpoint,
}

/// One completed or superseded worker request.
#[derive(Debug)]
pub struct EggpoolResult {
    /// Worker generation for stale-result rejection.
    pub generation: u64,
    /// Period requested by this attempt.
    pub period: EggpoolPeriod,
    /// Request start time.
    #[allow(dead_code)] // Retained for refresh-latency diagnostics.
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
    client: eggfetch_core::Client,
    env_lookup: EnvLookup,
}

fn eggfetch_timeout(timeout: Duration) -> eggfetch_core::Timeout {
    eggfetch_core::Timeout {
        pool: Some(timeout),
        connect: Some(timeout),
        write: Some(timeout),
        read: Some(timeout),
        total: Some(timeout),
    }
}

impl EggpoolClient {
    /// Build a client with a bounded idle pool.
    ///
    /// Redirect following is not compiled in the lean `standard-http1`
    /// profile, so 3xx responses pass through directly.
    #[must_use]
    pub fn new(timeout: Duration) -> Self {
        Self::with_env_lookup(timeout, Arc::new(|name| env::var_os(name)))
    }

    fn with_env_lookup(timeout: Duration, env_lookup: EnvLookup) -> Self {
        let client = eggfetch_core::Client::builder()
            .timeout(eggfetch_timeout(timeout))
            .max_idle_connections_per_host(2)
            .max_decoded_body_size(MAX_RESPONSE_BYTES)
            .build();
        Self { client, env_lookup }
    }

    /// Fetch one validated summary. No automatic retry or alternate endpoint
    /// is attempted.
    pub async fn fetch(
        &self,
        endpoint: &EggpoolEntry,
        period: EggpoolPeriod,
    ) -> EggpoolFetchOutcome {
        let auth_token = match endpoint.api_key_env.as_deref() {
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
            return EggpoolFetchOutcome::InvalidEndpoint;
        };
        let url = url.as_str().to_string();
        let Ok(mut builder) = self
            .client
            .get(&url)
            .map(|b| b.max_decoded_body_size(MAX_RESPONSE_BYTES))
        else {
            return EggpoolFetchOutcome::InvalidEndpoint;
        };
        if let Some(token) = auth_token {
            let Ok(auth) = AuthScheme::bearer(token) else {
                // The configured secret is present but contains characters
                // that cannot be encoded into a valid `Authorization` header
                // value; surface it as an invalid summary rather than a
                // missing-key misclassification. The secret is dropped here
                // and never retained in the outcome.
                return EggpoolFetchOutcome::InvalidSummary;
            };
            builder = builder.auth(auth);
        }

        let mut response = match builder.send_detailed().await {
            Ok(response) => response,
            Err(failure) => return classify_request_error(&failure),
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
        let body = match response.bytes().await {
            Ok(bytes) => bytes,
            Err(error) => {
                if matches!(error, EggfetchError::DecodedBodyTooLarge) {
                    return EggpoolFetchOutcome::BodyTooLarge;
                }
                if matches!(
                    error,
                    EggfetchError::Timeout { .. } | EggfetchError::TransportIoTimeout { .. }
                ) {
                    // 0.1.7 enforces `Timeout.total` as one absolute
                    // wall-clock deadline through response-body EOF, so a
                    // body-stage timeout honors the configured
                    // whole-request deadline category.
                    return EggpoolFetchOutcome::Timeout;
                }
                return EggpoolFetchOutcome::NetworkError;
            }
        };
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
    let host = endpoint
        .host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(&endpoint.host);
    let host = crate::endpoint::bracketed_host(host).map_err(|_| ())?;
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.clone()
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

fn classify_request_error(failure: &RequestFailure) -> EggpoolFetchOutcome {
    if matches!(failure.error(), EggfetchError::DecodedBodyTooLarge) {
        return EggpoolFetchOutcome::BodyTooLarge;
    }
    if failure.is_timeout() {
        return EggpoolFetchOutcome::Timeout;
    }
    match failure.network_failure_kind() {
        Some(NetworkFailureKind::Dns) => EggpoolFetchOutcome::DnsFailure,
        Some(NetworkFailureKind::ConnectionRefused) => EggpoolFetchOutcome::ConnectionRefused,
        Some(NetworkFailureKind::Connect | _) | None => EggpoolFetchOutcome::NetworkError,
    }
}

/// The single latest `EggPool` worker intent owned by the reducer.
///
/// Plan 151: the pane already tracks `active`, `period`, and a refresh
/// nonce in one place, so the worker contract is one retained latest
/// desired state rather than a lossy command queue. Publication is
/// synchronous and capacity-free, therefore it can never drop a
/// state-changing transition and never stalls the input path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EggpoolDesiredState {
    /// Whether the `EggPool` pane is currently visible.
    pub active: bool,
    /// The selected rolling window.
    pub period: EggpoolPeriod,
    /// Refresh nonce owned by the reducer; never incremented by the worker.
    pub generation: u64,
}

impl EggpoolDesiredState {
    /// The inactive desired state published before any activation.
    const INACTIVE: Self = Self {
        active: false,
        period: EggpoolPeriod::Hour,
        generation: 0,
    };
}

/// The worker control channel closed: no live worker can receive intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EggpoolWorkerClosed;

impl std::fmt::Display for EggpoolWorkerClosed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EggPool worker control channel is closed")
    }
}

impl std::error::Error for EggpoolWorkerClosed {}

/// Nonblocking publisher of the latest desired `EggPool` worker state.
///
/// A watch sender never waits for capacity and never queues: it retains
/// the newest value until the worker observes it, and it still reports a
/// closed worker so the pane can show `WorkerUnavailable`.
#[derive(Debug, Clone)]
pub struct EggpoolControl {
    sender: tokio::sync::watch::Sender<EggpoolDesiredState>,
}

impl EggpoolControl {
    /// Publish the newest desired state. Never blocks, never drops a
    /// state-changing transition, and never overwrites a newer intent.
    pub fn publish(&self, desired: EggpoolDesiredState) -> Result<(), EggpoolWorkerClosed> {
        self.sender.send(desired).map_err(|_| EggpoolWorkerClosed)
    }

    /// Return the newest published desired state.
    ///
    /// This is the retained value the worker converges onto; observing it
    /// requires no second control path.
    #[must_use]
    pub fn published(&self) -> EggpoolDesiredState {
        *self.sender.borrow()
    }
}

/// Handle for the optional worker's control and result channels.
pub struct EggpoolWorker {
    /// Publish the latest desired worker state.
    pub control: EggpoolControl,
    /// Receive completed results.
    pub results: tokio::sync::mpsc::Receiver<EggpoolResult>,
}

/// Start one worker for one configured `EggPool` endpoint.
pub fn spawn_worker(
    client: EggpoolClient,
    endpoint: EggpoolEntry,
    cancel: tokio_util::sync::CancellationToken,
) -> EggpoolWorker {
    spawn_worker_with_clock(client, endpoint, cancel, RealClock)
}

/// [`spawn_worker`] with an injected clock so tests can pin result
/// timestamps deterministically.
pub fn spawn_worker_with_clock<C>(
    client: EggpoolClient,
    endpoint: EggpoolEntry,
    cancel: tokio_util::sync::CancellationToken,
    clock: C,
) -> EggpoolWorker
where
    C: Clock + Clone + Send + 'static,
{
    let (control_tx, mut control_rx) = tokio::sync::watch::channel(EggpoolDesiredState::INACTIVE);
    let (result_tx, result_rx) = tokio::sync::mpsc::channel(4);
    tokio::spawn(async move {
        let mut worker = EggpoolWorkerState::new();
        loop {
            tokio::select! {
                () = cancel.cancelled() => {
                    worker.abort_request();
                    break;
                }
                changed = control_rx.changed() => {
                    if changed.is_err() {
                        // No publisher remains, so no desired state can
                        // supersede this one. Cancel is not required here.
                        worker.abort_request();
                        break;
                    }
                    if worker.converge(*control_rx.borrow_and_update()) {
                        worker.start_request(&client, &endpoint, &clock);
                    }
                }
                _ = async {
                    let deadline = worker.next_refresh_at?;
                    tokio::time::sleep_until(deadline).await;
                    Some(())
                }, if worker.next_refresh_at.is_some() => {
                    // Adopt the newest published state first: a pending
                    // deactivation, period change, or manual refresh must
                    // win over this passive deadline, and a passive
                    // refresh must never fetch superseded intent.
                    worker.next_refresh_at = None;
                    let desired = *control_rx.borrow_and_update();
                    let superseded = worker.converge(desired);
                    if superseded || (desired.active && worker.request.is_none()) {
                        worker.start_request(&client, &endpoint, &clock);
                    }
                }
                completed = async {
                    match worker.request.as_mut() {
                        Some(handle) => Some(handle.await),
                        None => None,
                    }
                }, if worker.request.is_some() => {
                    worker.request = None;
                    let (generation, period, started_at, outcome) = match completed {
                        Some(Ok(tuple)) => tuple,
                        // A panicked fetch task must still deliver a
                        // result so the pane's Refreshing status
                        // resolves instead of stalling until the next
                        // periodic refresh. The in-flight request always
                        // carries the newest desired generation and
                        // period, so those are safe to reuse here.
                        Some(Err(_)) | None => (
                            worker.desired.generation,
                            worker.desired.period,
                            clock.now(),
                            EggpoolFetchOutcome::NetworkError,
                        ),
                    };
                    let _ = result_tx.send(EggpoolResult { generation, period, started_at, completed_at: clock.now(), outcome }).await;
                    if worker.desired.active {
                        // Two clocks: `started_at`/`completed_at` use wall-clock
                        // `now()` while the refresh deadline uses the Tokio
                        // timer clock `tokio_now()`. A fake clock must advance
                        // the wall clock for timestamps and rely on the runtime
                        // clock for deadlines (see `FakeClock`); advancing one
                        // without the other breaks refresh scheduling silently.
                        worker.next_refresh_at = Some(clock.tokio_now() + REFRESH_INTERVAL);
                    }
                }
            }
        }
    });
    EggpoolWorker {
        control: EggpoolControl { sender: control_tx },
        results: result_rx,
    }
}

/// One completed `EggPool` request.
type RequestTask = tokio::task::JoinHandle<(u64, EggpoolPeriod, Instant, EggpoolFetchOutcome)>;

/// Plan 151: worker-side convergence onto the latest desired state.
///
/// Desired states the worker never observed individually are coalesced:
/// only the newest state is authoritative.
struct EggpoolWorkerState {
    /// The newest desired state this worker has converged onto.
    desired: EggpoolDesiredState,
    /// At most one in-flight request.
    request: Option<RequestTask>,
    /// Request-relative passive deadline, armed only after completion.
    next_refresh_at: Option<tokio::time::Instant>,
}

impl EggpoolWorkerState {
    fn new() -> Self {
        Self {
            desired: EggpoolDesiredState::INACTIVE,
            request: None,
            next_refresh_at: None,
        }
    }

    /// Converge onto `desired`; return `true` when a request for it must
    /// start immediately.
    ///
    /// - inactive desired state aborts in-flight work, clears the passive
    ///   deadline, and never emits a synthetic result;
    /// - a newly active, changed period, or changed generation aborts
    ///   obsolete work and starts exactly one request for the newest state;
    /// - an unchanged desired state leaves current work untouched.
    fn converge(&mut self, desired: EggpoolDesiredState) -> bool {
        let supersedes = desired.active
            && (!self.desired.active
                || self.desired.period != desired.period
                || self.desired.generation != desired.generation);
        self.desired = desired;
        if !desired.active {
            self.next_refresh_at = None;
            self.abort_request();
            return false;
        }
        if supersedes {
            self.abort_request();
            self.next_refresh_at = None;
            return true;
        }
        false
    }

    fn start_request<C: Clock + Clone + Send + 'static>(
        &mut self,
        client: &EggpoolClient,
        endpoint: &EggpoolEntry,
        clock: &C,
    ) {
        self.request = Some(spawn_request(
            client,
            endpoint,
            self.desired.period,
            self.desired.generation,
            clock,
        ));
    }

    fn abort_request(&mut self) {
        if let Some(request) = self.request.take() {
            request.abort();
        }
    }
}

fn spawn_request<C: Clock + Clone + Send + 'static>(
    client: &EggpoolClient,
    endpoint: &EggpoolEntry,
    period: EggpoolPeriod,
    generation: u64,
    clock: &C,
) -> RequestTask {
    let client = client.clone();
    let endpoint = endpoint.clone();
    let clock = clock.clone();
    tokio::spawn(async move {
        let started_at = clock.now();
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
    use tokio::sync::mpsc;

    pub(super) fn endpoint(port: u16, api_key_env: Option<&str>) -> EggpoolEntry {
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
    async fn header_with_control_chars_is_invalid_summary_not_missing_key() {
        // A present-but-unencodable secret must surface as InvalidSummary
        // rather than being misreported as a missing API key.
        let client = EggpoolClient::with_env_lookup(
            Duration::from_secs(2),
            Arc::new(|_| Some(OsString::from("bad\nvalue"))),
        );
        let result = client
            .fetch(&endpoint(1, Some("KEY")), EggpoolPeriod::Hour)
            .await;
        assert_eq!(result, EggpoolFetchOutcome::InvalidSummary);
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

    fn app_config(port: u16) -> crate::config::Config {
        crate::config::Config {
            eggpool: Some(endpoint(port, None)),
            ..crate::config::Config::default()
        }
    }

    fn desired(period: EggpoolPeriod, generation: u64) -> EggpoolDesiredState {
        EggpoolDesiredState {
            active: true,
            period,
            generation,
        }
    }

    fn inactive(period: EggpoolPeriod, generation: u64) -> EggpoolDesiredState {
        EggpoolDesiredState {
            active: false,
            period,
            generation,
        }
    }

    /// A real-clock watchdog so a broken worker fails a test instead of
    /// hanging the harness.
    ///
    /// It deliberately uses an OS thread rather than a Tokio timer: in a
    /// paused-time test, arming a timer would let virtual clock
    /// auto-advance race the loopback round trip.
    fn watchdog() -> tokio::sync::oneshot::Receiver<()> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(15));
            // The receiver may already be gone; that is fine.
            let _ = sender.send(());
        });
        receiver
    }

    /// A loopback summary server that records every request path and holds
    /// every response until the returned gate opens.
    ///
    /// Connections are keep-alive and served concurrently, matching how a
    /// real HTTP server behaves when Gregg aborts a superseded request and
    /// immediately issues another one.
    async fn server_gated() -> (
        u16,
        mpsc::Receiver<String>,
        tokio::sync::watch::Sender<bool>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (request_tx, request_rx) = mpsc::channel(64);
        let (gate_tx, gate_rx) = tokio::sync::watch::channel(false);
        let ordinal = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let request_tx = request_tx.clone();
                let mut gate_rx = gate_rx.clone();
                let ordinal = Arc::clone(&ordinal);
                tokio::spawn(async move {
                    let mut buffered = vec![0; 8192];
                    let mut used = 0;
                    loop {
                        // Read one complete request head. `GET` carries no
                        // body, so the header terminator frames the request.
                        let head = loop {
                            if let Some(at) = find_header_end(&buffered[..used]) {
                                break String::from_utf8_lossy(&buffered[..at]).into_owned();
                            }
                            if used == buffered.len() {
                                return;
                            }
                            let Ok(count) = stream.read(&mut buffered[used..]).await else {
                                return;
                            };
                            if count == 0 {
                                return;
                            }
                            used += count;
                        };
                        let Some(path) = head
                            .lines()
                            .next()
                            .unwrap_or_default()
                            .split_whitespace()
                            .nth(1)
                            .map(str::to_string)
                        else {
                            return;
                        };
                        // A superseded request may already be abandoned, so
                        // recorded paths are not proof of a delivered
                        // result; convergence is asserted from results.
                        let period = path.split("period=").nth(1).unwrap_or("1h").to_string();
                        let _ = request_tx.send(path).await;
                        // Hold the response until the gate opens.
                        while !*gate_rx.borrow_and_update() {
                            if gate_rx.changed().await.is_err() {
                                return;
                            }
                        }
                        let ordinal = ordinal.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                        let summary = format!(
                            "{{\"period\":\"{period}\",\"accounted_tokens\":{ordinal},\"cache_read_ratio\":null,\"tokens_per_second\":1.5,\"avg_ttft_ms\":12.0,\"streamed_requests\":0}}"
                        );
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{summary}",
                            summary.len()
                        );
                        if stream.write_all(response.as_bytes()).await.is_err() {
                            return;
                        }
                        used = 0;
                    }
                });
            }
        });
        (port, request_rx, gate_tx, task)
    }

    fn find_header_end(bytes: &[u8]) -> Option<usize> {
        bytes
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|at| at + 4)
    }

    fn worker_for(port: u16, cancel: &tokio_util::sync::CancellationToken) -> EggpoolWorker {
        spawn_worker(
            EggpoolClient::new(Duration::from_secs(10)),
            endpoint(port, None),
            cancel.clone(),
        )
    }

    async fn next_request(requests: &mut mpsc::Receiver<String>) -> String {
        let watchdog = watchdog();
        tokio::select! {
            biased;
            path = requests.recv() => path.expect("the request channel stays open"),
            _ = watchdog => panic!("no EggPool summary request was issued"),
        }
    }

    async fn next_result(worker: &mut EggpoolWorker) -> EggpoolResult {
        let watchdog = watchdog();
        tokio::select! {
            biased;
            result = worker.results.recv() => result.expect("the result channel stays open"),
            _ = watchdog => panic!("no EggPool worker result was delivered"),
        }
    }

    async fn stop(
        worker: &mut EggpoolWorker,
        cancel: &tokio_util::sync::CancellationToken,
        server: tokio::task::JoinHandle<()>,
    ) {
        cancel.cancel();
        // Cancellation needs no queued command and terminates promptly.
        assert!(worker.results.recv().await.is_none());
        server.abort();
        let _ = server.await;
    }

    #[tokio::test(start_paused = true)]
    async fn worker_passive_refresh_keeps_generation_and_updates_state() {
        let (port, mut requests, gate, server_task) = server_gated().await;
        gate.send(true).ok();
        let cancel = tokio_util::sync::CancellationToken::new();
        let mut worker = worker_for(port, &cancel);
        let mut app = crate::state::AppState::from_config(&app_config(port));
        app.begin_eggpool_request();
        worker
            .control
            .publish(app.eggpool_desired_state().unwrap())
            .unwrap();

        assert_eq!(
            next_request(&mut requests).await,
            "/api/stats/summary?period=1h"
        );
        let first = next_result(&mut worker).await;
        assert_eq!((first.generation, first.period), (1, EggpoolPeriod::Hour));
        app.apply_eggpool_result(&first);
        assert_eq!(
            app.eggpool
                .as_ref()
                .unwrap()
                .summary
                .as_ref()
                .unwrap()
                .accounted_tokens,
            1
        );

        tokio::time::advance(REFRESH_INTERVAL).await;
        tokio::task::yield_now().await;
        assert_eq!(
            next_request(&mut requests).await,
            "/api/stats/summary?period=1h"
        );
        let passive = next_result(&mut worker).await;
        // A passive refresh reuses the current generation; the worker
        // never increments the reducer-owned nonce.
        assert_eq!(
            (passive.generation, passive.period),
            (1, EggpoolPeriod::Hour)
        );
        app.apply_eggpool_result(&passive);
        assert_eq!(
            app.eggpool
                .as_ref()
                .unwrap()
                .summary
                .as_ref()
                .unwrap()
                .accounted_tokens,
            2
        );

        stop(&mut worker, &cancel, server_task).await;
    }

    #[tokio::test(start_paused = true)]
    async fn worker_deadlines_are_relative_to_activation_triggers_and_deactivation() {
        let (port, mut requests, gate, server_task) = server_gated().await;
        gate.send(true).ok();
        let cancel = tokio_util::sync::CancellationToken::new();
        let mut worker = worker_for(port, &cancel);
        // The passive deadline is relative to the activation trigger, not
        // to worker construction.
        tokio::time::advance(Duration::from_secs(59)).await;
        worker
            .control
            .publish(desired(EggpoolPeriod::Hour, 1))
            .unwrap();
        assert_eq!(
            next_request(&mut requests).await,
            "/api/stats/summary?period=1h"
        );
        let _ = next_result(&mut worker).await;
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert!(requests.try_recv().is_err());

        tokio::time::advance(Duration::from_secs(59)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            next_request(&mut requests).await,
            "/api/stats/summary?period=1h"
        );
        let _ = next_result(&mut worker).await;

        // A manual refresh at an unchanged period is still observable
        // because the generation changed.
        tokio::time::advance(Duration::from_secs(59)).await;
        worker
            .control
            .publish(desired(EggpoolPeriod::Hour, 2))
            .unwrap();
        assert_eq!(
            next_request(&mut requests).await,
            "/api/stats/summary?period=1h"
        );
        let refreshed = next_result(&mut worker).await;
        assert_eq!(
            (refreshed.generation, refreshed.period),
            (2, EggpoolPeriod::Hour)
        );
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert!(requests.try_recv().is_err());
        tokio::time::advance(Duration::from_secs(59)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            next_request(&mut requests).await,
            "/api/stats/summary?period=1h"
        );
        let _ = next_result(&mut worker).await;

        tokio::time::advance(Duration::from_secs(59)).await;
        worker
            .control
            .publish(desired(EggpoolPeriod::Day, 3))
            .unwrap();
        assert_eq!(
            next_request(&mut requests).await,
            "/api/stats/summary?period=24h"
        );
        let changed = next_result(&mut worker).await;
        assert_eq!(
            (changed.generation, changed.period),
            (3, EggpoolPeriod::Day)
        );
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert!(requests.try_recv().is_err());
        tokio::time::advance(Duration::from_secs(59)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            next_request(&mut requests).await,
            "/api/stats/summary?period=24h"
        );
        let _ = next_result(&mut worker).await;

        // Leaving the pane clears the deadline without fabricating a new
        // request generation.
        worker
            .control
            .publish(inactive(EggpoolPeriod::Day, 3))
            .unwrap();
        tokio::time::advance(Duration::from_secs(120)).await;
        tokio::task::yield_now().await;
        assert!(requests.try_recv().is_err());

        stop(&mut worker, &cancel, server_task).await;
    }

    #[tokio::test(start_paused = true)]
    async fn worker_passive_refresh_interval_starts_after_completion() {
        let (port, mut requests, gate, server_task) = server_gated().await;
        let cancel = tokio_util::sync::CancellationToken::new();
        let mut worker = worker_for(port, &cancel);
        worker
            .control
            .publish(desired(EggpoolPeriod::Hour, 1))
            .unwrap();
        assert_eq!(
            next_request(&mut requests).await,
            "/api/stats/summary?period=1h"
        );

        // The deadline is armed only after the request completes.
        tokio::time::advance(REFRESH_INTERVAL).await;
        tokio::task::yield_now().await;
        assert!(requests.try_recv().is_err());

        gate.send(true).ok();
        let _ = next_result(&mut worker).await;
        tokio::time::advance(Duration::from_secs(59)).await;
        tokio::task::yield_now().await;
        assert!(requests.try_recv().is_err());

        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            next_request(&mut requests).await,
            "/api/stats/summary?period=1h"
        );

        stop(&mut worker, &cancel, server_task).await;
    }

    #[tokio::test(start_paused = true)]
    async fn worker_cancellation_aborts_an_in_flight_request() {
        let (port, mut requests, gate, server_task) = server_gated().await;
        let cancel = tokio_util::sync::CancellationToken::new();
        let mut worker = worker_for(port, &cancel);
        worker
            .control
            .publish(desired(EggpoolPeriod::Hour, 1))
            .unwrap();
        assert_eq!(
            next_request(&mut requests).await,
            "/api/stats/summary?period=1h"
        );
        stop(&mut worker, &cancel, server_task).await;
        gate.send(true).ok();
    }

    #[tokio::test(start_paused = true)]
    async fn worker_deactivation_aborts_an_in_flight_request() {
        let (port, mut requests, gate, server_task) = server_gated().await;
        let cancel = tokio_util::sync::CancellationToken::new();
        let mut worker = worker_for(port, &cancel);
        worker
            .control
            .publish(desired(EggpoolPeriod::Hour, 1))
            .unwrap();
        assert_eq!(
            next_request(&mut requests).await,
            "/api/stats/summary?period=1h"
        );
        worker
            .control
            .publish(inactive(EggpoolPeriod::Hour, 1))
            .unwrap();
        // The aborted fetch must not deliver a result.
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert!(worker.results.try_recv().is_err());
        // Periodic refresh stays disabled while deactivated.
        tokio::time::advance(REFRESH_INTERVAL * 2).await;
        tokio::task::yield_now().await;
        assert!(requests.try_recv().is_err());
        assert!(worker.results.try_recv().is_err());

        stop(&mut worker, &cancel, server_task).await;
        gate.send(true).ok();
    }

    #[tokio::test(start_paused = true)]
    async fn worker_panic_in_fetch_task_still_delivers_a_result() {
        // The injected env lookup panics inside the spawned fetch task,
        // so the request completes as a JoinError instead of an outcome.
        let client = EggpoolClient::with_env_lookup(
            Duration::from_secs(10),
            Arc::new(|_name: &str| -> Option<OsString> { panic!("injected fetch panic") }),
        );
        let cancel = tokio_util::sync::CancellationToken::new();
        let mut worker = spawn_worker(client, endpoint(1, Some("KEY")), cancel.clone());
        worker
            .control
            .publish(desired(EggpoolPeriod::Hour, 1))
            .unwrap();
        let result = next_result(&mut worker).await;
        assert_eq!(result.outcome, EggpoolFetchOutcome::NetworkError);
        assert_eq!((result.generation, result.period), (1, EggpoolPeriod::Hour));
        cancel.cancel();
    }

    #[tokio::test(start_paused = true)]
    async fn worker_result_timestamps_come_from_the_injected_clock() {
        let (port, mut requests, gate, server_task) = server_gated().await;
        gate.send(true).ok();
        let cancel = tokio_util::sync::CancellationToken::new();
        let anchor = Instant::now();
        let mut worker = spawn_worker_with_clock(
            EggpoolClient::new(Duration::from_secs(10)),
            endpoint(port, None),
            cancel.clone(),
            crate::clock::FakeClock::new(anchor),
        );
        worker
            .control
            .publish(desired(EggpoolPeriod::Hour, 1))
            .unwrap();
        assert_eq!(
            next_request(&mut requests).await,
            "/api/stats/summary?period=1h"
        );
        let result = next_result(&mut worker).await;
        assert!(matches!(result.outcome, EggpoolFetchOutcome::Online(_)));
        // The fake clock never advances, so both timestamps pin to its
        // anchor instead of wall-clock instants.
        assert_eq!(result.started_at, anchor);
        assert_eq!(result.completed_at, anchor);
        stop(&mut worker, &cancel, server_task).await;
    }

    #[tokio::test(start_paused = true)]
    async fn rapid_publication_never_waits_for_worker_capacity() {
        let (port, mut requests, gate, server_task) = server_gated().await;
        let cancel = tokio_util::sync::CancellationToken::new();
        let mut worker = worker_for(port, &cancel);
        // The activation leaves a request in flight for the whole burst:
        // the pressure the old bounded queue dropped commands under.
        worker
            .control
            .publish(desired(EggpoolPeriod::Hour, 1))
            .unwrap();
        assert_eq!(
            next_request(&mut requests).await,
            "/api/stats/summary?period=1h"
        );

        let burst = async {
            for generation in 2..10_000u64 {
                worker
                    .control
                    .publish(desired(EggpoolPeriod::Month, generation))
                    .expect("a live worker control channel");
            }
        };
        tokio::select! {
            biased;
            () = tokio::task::yield_now() => {
                panic!("desired-state publication blocked on the worker");
            }
            () = burst => {}
        }
        // The retained latest value is the newest publication.
        assert_eq!(
            worker.control.published(),
            desired(EggpoolPeriod::Month, 9_999)
        );

        gate.send(true).ok();
        let result = next_result(&mut worker).await;
        assert_eq!(
            (result.generation, result.period),
            (9_999, EggpoolPeriod::Month)
        );
        // Intermediate states are coalesced rather than replayed, and no
        // passive request follows immediately.
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert!(worker.results.try_recv().is_err());

        stop(&mut worker, &cancel, server_task).await;
    }

    #[tokio::test(start_paused = true)]
    async fn worker_held_in_flight_converges_on_the_final_desired_state() {
        let (port, mut requests, gate, server_task) = server_gated().await;
        let cancel = tokio_util::sync::CancellationToken::new();
        let mut worker = worker_for(port, &cancel);
        worker
            .control
            .publish(desired(EggpoolPeriod::Hour, 1))
            .unwrap();
        assert_eq!(
            next_request(&mut requests).await,
            "/api/stats/summary?period=1h"
        );

        // Supersede the in-flight request while its response is still
        // held; obsolete work is aborted and the newest state is served.
        worker
            .control
            .publish(desired(EggpoolPeriod::Day, 2))
            .unwrap();
        worker
            .control
            .publish(desired(EggpoolPeriod::Week, 3))
            .unwrap();
        assert_eq!(
            next_request(&mut requests).await,
            "/api/stats/summary?period=7d"
        );
        gate.send(true).ok();
        let result = next_result(&mut worker).await;
        assert_eq!((result.generation, result.period), (3, EggpoolPeriod::Week));
        assert!(matches!(result.outcome, EggpoolFetchOutcome::Online(_)));
        // The abandoned requests never mutate visible state.
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert!(worker.results.try_recv().is_err());

        stop(&mut worker, &cancel, server_task).await;
    }

    #[tokio::test(start_paused = true)]
    async fn rapid_period_changes_converge_on_the_final_period_and_generation() {
        let (port, mut requests, gate, server_task) = server_gated().await;
        let cancel = tokio_util::sync::CancellationToken::new();
        let mut worker = worker_for(port, &cancel);
        for (generation, period) in [
            (1, EggpoolPeriod::Hour),
            (2, EggpoolPeriod::Day),
            (3, EggpoolPeriod::Week),
            (4, EggpoolPeriod::Month),
        ] {
            worker.control.publish(desired(period, generation)).unwrap();
        }

        // The newest state is authoritative; intermediate requests are
        // optional, but every delivered result belongs to a published state
        // and the newest state must be served last.
        let mut requests_seen = 0;
        let mut final_result = None;
        while final_result.is_none() {
            let path = next_request(&mut requests).await;
            requests_seen += 1;
            assert!(
                matches!(
                    path.as_str(),
                    "/api/stats/summary?period=1h"
                        | "/api/stats/summary?period=24h"
                        | "/api/stats/summary?period=7d"
                        | "/api/stats/summary?period=30d"
                ),
                "unexpected request {path}"
            );
            assert!(
                requests_seen <= 4,
                "no more than one request per published period: {requests_seen}"
            );
            // Answer every request once the worker has served one, so a
            // superseded request cannot hold the newest one hostage.
            gate.send(true).ok();
            let result = next_result(&mut worker).await;
            assert!(
                (1..=4).contains(&result.generation),
                "undelivered generation {}",
                result.generation
            );
            if result.generation == 4 {
                final_result = Some(result);
            }
        }
        let final_result = final_result.expect("a newest-generation result");
        assert_eq!(final_result.period, EggpoolPeriod::Month);
        assert!(matches!(
            final_result.outcome,
            EggpoolFetchOutcome::Online(_)
        ));

        // The converged result is not followed by an immediate extra
        // request; the passive deadline is 60 seconds.
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert!(requests.try_recv().is_err());

        stop(&mut worker, &cancel, server_task).await;
    }

    #[tokio::test(start_paused = true)]
    async fn activation_then_deactivation_under_pressure_arms_no_passive_refresh() {
        let (port, mut requests, gate, server_task) = server_gated().await;
        let cancel = tokio_util::sync::CancellationToken::new();
        let mut worker = worker_for(port, &cancel);
        worker
            .control
            .publish(desired(EggpoolPeriod::Hour, 1))
            .unwrap();
        assert_eq!(
            next_request(&mut requests).await,
            "/api/stats/summary?period=1h"
        );

        for (generation, period) in [
            (2, EggpoolPeriod::Day),
            (3, EggpoolPeriod::Week),
            (4, EggpoolPeriod::Month),
        ] {
            worker.control.publish(desired(period, generation)).unwrap();
        }
        // Leaving the the pane converges the worker to inactive even
        // while earlier requests are still being superseded.
        worker
            .control
            .publish(inactive(EggpoolPeriod::Month, 4))
            .unwrap();
        gate.send(true).ok();

        // No passive request may appear after leaving the pane, and no
        // synthetic result may be emitted for deactivation.
        tokio::time::advance(Duration::from_secs(120)).await;
        tokio::task::yield_now().await;
        assert!(requests.try_recv().is_err());
        assert!(worker.results.try_recv().is_err());

        stop(&mut worker, &cancel, server_task).await;
    }

    #[tokio::test(start_paused = true)]
    async fn closed_control_channel_reports_a_missing_worker() {
        let cancel = tokio_util::sync::CancellationToken::new();
        let mut worker = worker_for(1, &cancel);
        cancel.cancel();
        // The worker task owns the only receiver; once it exits, the
        // control channel is closed rather than silently accepting intent.
        assert!(worker.results.recv().await.is_none());
        assert_eq!(
            worker.control.publish(desired(EggpoolPeriod::Hour, 1)),
            Err(EggpoolWorkerClosed)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn unconfigured_state_has_no_worker_control_or_request() {
        // Invariant 8: no worker, control channel, timer, or request
        // exists when EggPool is not configured.
        let app = crate::state::AppState::from_config(&crate::config::Config::default());
        assert!(app.eggpool.is_none());
        assert!(app.eggpool_desired_state().is_none());
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

    #[test]
    fn summary_url_normalizes_bracketed_ipv6() {
        let endpoint = EggpoolEntry {
            host: "[2001:db8::1]".into(),
            port: 8080,
            ..endpoint(8080, None)
        };
        let url = summary_url(&endpoint, EggpoolPeriod::Hour).unwrap();
        assert_eq!(
            url.as_str(),
            "http://[2001:db8::1]:8080/api/stats/summary?period=1h"
        );
    }

    #[test]
    fn summary_host_normalizes_ipv6_zone_identifier() {
        let endpoint = EggpoolEntry {
            host: "fe80::1%eth0".into(),
            port: 11300,
            ..endpoint(11300, None)
        };
        let host = crate::endpoint::bracketed_host(&endpoint.host).unwrap();
        assert_eq!(host, "fe80::1%25eth0");
    }

    #[tokio::test]
    async fn fetch_reports_invalid_ipv6_zone_url() {
        let endpoint = EggpoolEntry {
            host: "fe80::1%eth0".into(),
            port: 11300,
            ..endpoint(11300, None)
        };
        let outcome = EggpoolClient::new(Duration::from_secs(1))
            .fetch(&endpoint, EggpoolPeriod::Hour)
            .await;
        assert_eq!(outcome, EggpoolFetchOutcome::InvalidEndpoint);
    }

    #[test]
    fn eggfetch_timeout_preserves_whole_request_deadline() {
        let timeout = Duration::from_millis(1500);
        let configured = eggfetch_timeout(timeout);
        assert_eq!(configured.pool, Some(timeout));
        assert_eq!(configured.connect, Some(timeout));
        assert_eq!(configured.write, Some(timeout));
        assert_eq!(configured.read, Some(timeout));
        assert_eq!(configured.total, Some(timeout));
    }

    #[test]
    fn https_endpoint_url_is_representable() {
        use crate::config::EggpoolScheme;
        let endpoint = EggpoolEntry {
            scheme: EggpoolScheme::Https,
            host: "pool.example.com".into(),
            port: 8443,
            ..endpoint(8443, None)
        };
        let url = summary_url(&endpoint, EggpoolPeriod::Hour).unwrap();
        assert_eq!(
            url.as_str(),
            "https://pool.example.com:8443/api/stats/summary?period=1h"
        );
        // The shared HTTPS-capable client builds without disabling
        // certificate verification; runtime TLS uses the packaged WebPKI roots.
        let _client = EggpoolClient::new(Duration::from_secs(1));
    }

    #[tokio::test]
    async fn chunked_body_over_cap_is_body_too_large() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
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
            let first = vec![b'x'; MAX_RESPONSE_BYTES - 1024];
            let second = vec![b'x'; 2048];
            let header =
                "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
            stream.write_all(header.as_bytes()).await.unwrap();
            for chunk in [&first, &second] {
                let size_line = format!("{:x}\r\n", chunk.len());
                stream.write_all(size_line.as_bytes()).await.unwrap();
                stream.write_all(chunk).await.unwrap();
                stream.write_all(b"\r\n").await.unwrap();
            }
            stream.write_all(b"0\r\n\r\n").await.unwrap();
        });
        let outcome = EggpoolClient::new(Duration::from_secs(2))
            .fetch(&endpoint(port, None), EggpoolPeriod::Hour)
            .await;
        assert_eq!(outcome, EggpoolFetchOutcome::BodyTooLarge);
    }

    #[tokio::test]
    async fn timeout_before_headers_is_timeout() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 8192];
            let mut used = 0;
            loop {
                let count = stream.read(&mut request[used..]).await.unwrap();
                if count == 0 {
                    return;
                }
                used += count;
                if request[..used]
                    .windows(4)
                    .any(|window| window == b"\r\n\r\n")
                {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
                .await;
        });
        let outcome = EggpoolClient::new(Duration::from_millis(50))
            .fetch(&endpoint(port, None), EggpoolPeriod::Hour)
            .await;
        assert_eq!(outcome, EggpoolFetchOutcome::Timeout);
    }

    #[tokio::test]
    async fn body_stall_after_headers_is_timeout() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 8192];
            let mut used = 0;
            loop {
                let count = stream.read(&mut request[used..]).await.unwrap();
                if count == 0 {
                    return;
                }
                used += count;
                if request[..used]
                    .windows(4)
                    .any(|window| window == b"\r\n\r\n")
                {
                    break;
                }
            }
            // Headers complete at once; the declared 1 KiB body never
            // arrives, so body consumption exceeds the absolute total
            // deadline and must map to Timeout, not NetworkError.
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1024\r\nConnection: close\r\n\r\n")
                .await;
            tokio::time::sleep(Duration::from_secs(10)).await;
        });
        let outcome = EggpoolClient::new(Duration::from_millis(200))
            .fetch(&endpoint(port, None), EggpoolPeriod::Hour)
            .await;
        assert_eq!(outcome, EggpoolFetchOutcome::Timeout);
    }

    #[tokio::test]
    async fn closed_port_is_refused_or_network_error() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        // Use the same 5s deadline as the Systems poller so a slow
        // Windows refusal surfaces as typed evidence instead of a
        // whole-request timeout.
        let outcome = EggpoolClient::new(Duration::from_secs(5))
            .fetch(&endpoint(port, None), EggpoolPeriod::Hour)
            .await;
        assert!(
            matches!(
                outcome,
                EggpoolFetchOutcome::ConnectionRefused | EggpoolFetchOutcome::NetworkError
            ),
            "expected ConnectionRefused or NetworkError, got {outcome:?}"
        );
    }

    #[tokio::test]
    async fn redirect_is_not_followed() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let response =
            "HTTP/1.1 301 Moved Permanently\r\nContent-Length: 8\r\n\r\nredirect".to_string();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
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
        });
        let outcome = EggpoolClient::new(Duration::from_secs(2))
            .fetch(&endpoint(port, None), EggpoolPeriod::Hour)
            .await;
        assert_eq!(outcome, EggpoolFetchOutcome::HttpStatus(301));
    }
}
