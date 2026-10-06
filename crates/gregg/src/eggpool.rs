//! Bounded `EggPool` summary client and pane refresh worker.

use std::env;
use std::ffi::OsString;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eggfetch_core::{AuthScheme, Error as EggfetchError, NetworkFailureKind, RequestFailure};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::clock::{Clock, RealClock};
use crate::config::EggpoolEntry;

/// Summary decoded-body ceiling. The compact four-value payload stays far
/// below this bound.
const MAX_RESPONSE_BYTES: usize = 16 * 1024;
/// Status decoded-body ceiling, aligned with `EggPool`'s own bounded status
/// client. It is applied per request so the summary route keeps its 16 KiB
/// limit.
const MAX_STATUS_RESPONSE_BYTES: usize = 1024 * 1024;
/// Provider rows `EggPool` itself bounds in one status snapshot
/// (`MAX_STATUS_PROVIDERS`).
const MAX_STATUS_PROVIDER_ROWS: usize = 256;
/// Bounded provider identity length accepted before rendering, mirroring
/// `EggPool`'s own `MAX_PROVIDER_ID_CHARS` bound on bytes retained in status
/// output.
const MAX_PROVIDER_ID_BYTES: usize = 96;
/// Bounded reason-code length accepted before rendering, mirroring
/// `EggPool`'s own `MAX_REASON_CODE_CHARS` bound.
const MAX_STATUS_REASON_BYTES: usize = 64;
/// The only status schema version Gregg treats as authoritative.
const STATUS_SCHEMA_VERSION: u64 = 1;
const REFRESH_INTERVAL: Duration = Duration::from_secs(60);

/// The four fixed rolling windows supported by `EggPool`'s summary API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
///
/// Plan 164: `Serialize`/`Deserialize` let the client daemon publish the real
/// classification to frontends instead of a pre-rendered string, so the TUI
/// keeps choosing its own wording for a transport failure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

/// `EggPool`'s server-reported proxy health, independent of Gregg's local
/// worker lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EggpoolProxyHealth {
    /// The proxy reports itself ready to serve requests.
    Ready,
    /// The proxy is serving with reduced capability.
    Degraded,
    /// The proxy reports itself unable to serve requests.
    Unready,
}

impl EggpoolProxyHealth {
    /// The plain status word used as the primary signal.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Degraded => "degraded",
            Self::Unready => "unready",
        }
    }
}

/// `EggPool`'s server-reported provider health.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EggpoolProviderHealth {
    /// The provider is usable.
    Ready,
    /// The provider is usable with reduced capability.
    Degraded,
    /// The provider is currently unusable.
    Unavailable,
    /// The provider is administratively disabled.
    Disabled,
    /// `EggPool` does not know the provider state, including a value this
    /// client does not recognize.
    Unknown,
}

impl EggpoolProviderHealth {
    /// The plain status word used in the bounded provider count summary.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Degraded => "degraded",
            Self::Unavailable => "unavailable",
            Self::Disabled => "disabled",
            Self::Unknown => "unknown",
        }
    }

    fn from_wire(value: &str) -> Self {
        match value {
            "ready" => Self::Ready,
            "degraded" => Self::Degraded,
            "unavailable" => Self::Unavailable,
            "disabled" => Self::Disabled,
            _ => Self::Unknown,
        }
    }
}

/// `EggPool`'s most recent observation of one provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EggpoolProviderObservation {
    /// The most recent probe verified the provider.
    Verified,
    /// The most recent probe failed.
    Failed,
    /// The most recent observation is older than `EggPool` accepts.
    Stale,
    /// The provider has never been observed.
    Never,
}

impl EggpoolProviderObservation {
    fn from_wire(value: &str) -> Option<Self> {
        match value {
            "verified" => Some(Self::Verified),
            "failed" => Some(Self::Failed),
            "stale" => Some(Self::Stale),
            "never" => Some(Self::Never),
            _ => None,
        }
    }
}

/// One decoded provider row of bounded health context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EggpoolProviderRow {
    /// Bounded provider identity reported by `EggPool`.
    pub id: String,
    /// The provider health `EggPool` reports.
    pub status: EggpoolProviderHealth,
    /// The most recent observation, or `None` when absent or unrecognized.
    pub observation: Option<EggpoolProviderObservation>,
}

/// A validated, display-ready `EggPool` service-health snapshot.
///
/// This is `EggPool`'s own operational health. It never describes Gregg's
/// worker, and Gregg never infers it from a transport failure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EggpoolHealthSnapshot {
    /// The decoded schema version, always `1`.
    pub schema_version: u64,
    /// The proxy status `EggPool` reports.
    pub proxy: EggpoolProxyHealth,
    /// Whether `EggPool` reports the proxy as available.
    pub available: bool,
    /// Bounded reason code, when `EggPool` supplies one.
    pub reason_code: Option<String>,
    /// Uptime in seconds, when reported.
    pub uptime_seconds: Option<f64>,
    /// Model count, when reported.
    pub model_count: Option<u64>,
    /// Routable account count, when reported.
    pub routable_accounts: Option<u64>,
    /// Enabled account count, when reported.
    pub enabled_accounts: Option<u64>,
    /// Bounded provider context, never a drill-down table.
    pub providers: Vec<EggpoolProviderRow>,
}

impl EggpoolHealthSnapshot {
    /// Count providers per reported health, ordered ready, degraded,
    /// unavailable, disabled, unknown.
    #[must_use]
    pub fn provider_counts(&self) -> [usize; 5] {
        let mut counts = [0; 5];
        for row in &self.providers {
            let index = match row.status {
                EggpoolProviderHealth::Ready => 0,
                EggpoolProviderHealth::Degraded => 1,
                EggpoolProviderHealth::Unavailable => 2,
                EggpoolProviderHealth::Disabled => 3,
                EggpoolProviderHealth::Unknown => 4,
            };
            counts[index] += 1;
        }
        counts
    }
}

/// A safe, stable classification of one `EggPool` health read.
///
/// A transport failure is never reported as a `EggPool`-reported proxy
/// status: `EggPool`'s internal unavailable state is not emitted by this
/// endpoint, so a failed read is a local transport fact only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EggpoolHealthFetchOutcome {
    /// A validated health snapshot was received.
    Online(EggpoolHealthSnapshot),
    /// `EggPool` requires credentials this configuration cannot supply.
    AuthenticationRequired,
    /// The credentials lack permission.
    Forbidden,
    /// This `EggPool` does not expose a status route.
    Unsupported,
    /// The configured key cannot be encoded into a valid `Authorization` header.
    InvalidApiKey,
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
    /// The response exceeded the bounded status body limit.
    BodyTooLarge,
    /// The response was not valid JSON of the expected shape.
    DecodeError,
    /// The status schema version is not one this client understands.
    UnsupportedSchema,
    /// The decoded payload violated the bounded contract.
    InvalidStatus,
    /// The configured endpoint cannot be represented as a valid request URL.
    InvalidEndpoint,
}

/// One completed or superseded worker request.
///
/// The summary and health planes are independent: partial success is the
/// normal case and is never collapsed into a single success or failure.
#[derive(Debug)]
pub struct EggpoolResult {
    /// Worker generation for stale-result rejection.
    pub generation: u64,
    /// Period requested by this attempt. Health has no period; this
    /// generation owns both planes.
    pub period: EggpoolPeriod,
    /// Request start time.
    #[allow(dead_code)] // Retained for refresh-latency diagnostics.
    pub started_at: Instant,
    /// Request completion time.
    pub completed_at: Instant,
    /// Summary-plane outcome.
    pub summary: EggpoolFetchOutcome,
    /// Health-plane outcome.
    pub health: EggpoolHealthFetchOutcome,
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

    /// Resolve the configured request-local credential.
    ///
    /// A missing or empty environment variable is reported separately from a
    /// configured value that cannot be encoded, because the two planes treat
    /// an absent key differently: the summary route stops, while the status
    /// route may still be readable when the dashboard is public.
    fn credential(&self, endpoint: &EggpoolEntry) -> Result<Option<AuthScheme>, CredentialError> {
        let Some(name) = endpoint.api_key_env.as_deref() else {
            return Ok(None);
        };
        let Some(value) = (self.env_lookup)(name).filter(|value| !value.is_empty()) else {
            return Err(CredentialError::Missing(name.to_string()));
        };
        let Ok(value) = value.into_string() else {
            return Err(CredentialError::Unusable);
        };
        AuthScheme::bearer(value)
            .map(Some)
            .map_err(|_| CredentialError::Unusable)
    }

    /// Fetch one validated summary. No automatic retry or alternate endpoint
    /// is attempted.
    pub async fn fetch(
        &self,
        endpoint: &EggpoolEntry,
        period: EggpoolPeriod,
    ) -> EggpoolFetchOutcome {
        let auth = match self.credential(endpoint) {
            Ok(auth) => auth,
            // A present-but-unencodable secret is surfaced as an invalid
            // summary rather than a missing-key misclassification. The
            // secret is dropped here and never retained in the outcome.
            Err(CredentialError::Missing(name)) => return missing_key(&name),
            Err(CredentialError::Unusable) => return EggpoolFetchOutcome::InvalidSummary,
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
        if let Some(auth) = auth {
            builder = builder.auth(auth);
        }

        let response = match builder.send_detailed().await {
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
        let body = match read_body(response).await {
            Ok(body) => body,
            Err(failure) => return classify_body_error(&failure),
        };
        let Ok(wire) = serde_json::from_slice::<EggpoolSummaryWire>(&body) else {
            return EggpoolFetchOutcome::DecodeError;
        };
        normalize_summary(&wire, period).map_or(
            EggpoolFetchOutcome::InvalidSummary,
            EggpoolFetchOutcome::Online,
        )
    }

    /// Read `EggPool`'s schema-version-1 service-health snapshot.
    ///
    /// This is a separate current-health plane. It never changes summary
    /// metric meaning, and no outbound provider probe, quota use, or mutation
    /// is triggered: only `EggPool`'s own read-only status route is read.
    pub async fn fetch_health(&self, endpoint: &EggpoolEntry) -> EggpoolHealthFetchOutcome {
        // Unlike the summary route, an absent key still sends the request:
        // `EggPool` keeps `/api/status` authenticated even when dashboard
        // pages are public, and the server's own answer is authoritative.
        let auth = match self.credential(endpoint) {
            Ok(auth) => auth,
            Err(CredentialError::Missing(_)) => None,
            Err(CredentialError::Unusable) => {
                return EggpoolHealthFetchOutcome::InvalidApiKey;
            }
        };

        let Ok(url) = status_url(endpoint) else {
            return EggpoolHealthFetchOutcome::InvalidEndpoint;
        };
        let url = url.as_str().to_string();
        let Ok(mut builder) = self
            .client
            .get(&url)
            // A per-request ceiling raises only this route; the client-wide
            // default stays at the summary bound.
            .map(|b| b.max_decoded_body_size(MAX_STATUS_RESPONSE_BYTES))
        else {
            return EggpoolHealthFetchOutcome::InvalidEndpoint;
        };
        if let Some(auth) = auth {
            builder = builder.auth(auth);
        }

        let response = match builder.send_detailed().await {
            Ok(response) => response,
            Err(failure) => return classify_health_request_error(&failure),
        };
        let status = response.status().as_u16();
        if !response.status().is_success() {
            return match status {
                401 => EggpoolHealthFetchOutcome::AuthenticationRequired,
                403 => EggpoolHealthFetchOutcome::Forbidden,
                // An older `EggPool` without a status route is explicitly
                // unsupported, not a statistics failure.
                404 => EggpoolHealthFetchOutcome::Unsupported,
                status => EggpoolHealthFetchOutcome::HttpStatus(status),
            };
        }
        let body = match read_body(response).await {
            Ok(body) => body,
            Err(failure) => return classify_health_body_error(&failure),
        };
        let Ok(wire) = serde_json::from_slice::<EggpoolStatusWire>(&body) else {
            return EggpoolHealthFetchOutcome::DecodeError;
        };
        // A future schema is never treated as authoritative; it degrades to
        // an explicit unsupported health state and never invalidates the
        // summary plane.
        if wire.schema_version != STATUS_SCHEMA_VERSION {
            return EggpoolHealthFetchOutcome::UnsupportedSchema;
        }
        normalize_health(&wire).map_or(
            EggpoolHealthFetchOutcome::InvalidStatus,
            EggpoolHealthFetchOutcome::Online,
        )
    }
}

/// Why a configured credential could not be used for one request.
enum CredentialError {
    /// The configured environment variable is absent or empty.
    Missing(String),
    /// A present value cannot be encoded into a valid `Authorization` header.
    Unusable,
}

/// Consume one bounded response body, surfacing typed body-stage failures.
async fn read_body(mut response: eggfetch_core::Response) -> Result<Vec<u8>, EggfetchError> {
    response.bytes().await.map(Vec::from)
}

fn missing_key(name: &str) -> EggpoolFetchOutcome {
    EggpoolFetchOutcome::MissingApiKeyEnv {
        name: name.to_string(),
    }
}

/// Build the normalized `scheme://host:port` origin for one endpoint.
fn origin_prefix(endpoint: &EggpoolEntry) -> Result<String, ()> {
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
    Ok(format!("{}://{}:{}", endpoint.scheme, host, endpoint.port))
}

fn summary_url(endpoint: &EggpoolEntry, period: EggpoolPeriod) -> Result<Url, ()> {
    let mut url =
        Url::parse(&format!("{}/api/stats/summary", origin_prefix(endpoint)?)).map_err(|_| ())?;
    url.query_pairs_mut()
        .append_pair("period", period.api_value());
    Ok(url)
}

fn status_url(endpoint: &EggpoolEntry) -> Result<Url, ()> {
    Url::parse(&format!("{}/api/status", origin_prefix(endpoint)?)).map_err(|_| ())
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

/// The schema-version-1 `/api/status` payload Gregg decodes.
///
/// The decoded names and nesting mirror `EggPool`'s serialized
/// `ProxyStatusSnapshot` (see the canonical provenance recorded on the
/// `canonical_status_body` test fixture): account counts live under `proxy`,
/// and provider rows carry `provider_id` / `last_observation`. Only the fields
/// needed to validate the contract and render compact health are decoded;
/// unknown extra fields are ignored so a future `EggPool` build does not
/// invalidate the snapshot.
#[derive(Debug, Deserialize)]
struct EggpoolStatusWire {
    schema_version: u64,
    proxy: EggpoolProxyWire,
    #[serde(default)]
    providers: Vec<EggpoolProviderWire>,
}

#[derive(Debug, Deserialize)]
struct EggpoolProxyWire {
    status: String,
    #[serde(default)]
    available: bool,
    #[serde(default)]
    reason_code: Option<String>,
    #[serde(default)]
    uptime_seconds: Option<f64>,
    #[serde(default)]
    model_count: Option<u64>,
    #[serde(default)]
    routable_accounts: Option<u64>,
    #[serde(default)]
    enabled_accounts: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct EggpoolProviderWire {
    provider_id: String,
    /// Absent in a future build means the state is genuinely unknown, not
    /// a decode failure for the whole health plane.
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    last_observation: Option<String>,
}

/// Validate one schema-version-1 status payload against the bounded contract.
fn normalize_health(wire: &EggpoolStatusWire) -> Result<EggpoolHealthSnapshot, ()> {
    let proxy = match wire.proxy.status.as_str() {
        "ready" => EggpoolProxyHealth::Ready,
        "degraded" => EggpoolProxyHealth::Degraded,
        "unready" => EggpoolProxyHealth::Unready,
        _ => return Err(()),
    };
    if !bounded_optional(wire.proxy.reason_code.as_deref(), MAX_STATUS_REASON_BYTES) {
        return Err(());
    }
    if !wire
        .proxy
        .uptime_seconds
        .is_none_or(|value| value.is_finite() && value >= 0.0)
    {
        return Err(());
    }
    if wire.providers.len() > MAX_STATUS_PROVIDER_ROWS {
        return Err(());
    }
    let mut providers = Vec::with_capacity(wire.providers.len());
    for row in &wire.providers {
        if !bounded_optional(Some(row.provider_id.as_str()), MAX_PROVIDER_ID_BYTES) {
            return Err(());
        }
        providers.push(EggpoolProviderRow {
            id: row.provider_id.clone(),
            status: row.status.as_deref().map_or(
                EggpoolProviderHealth::Unknown,
                EggpoolProviderHealth::from_wire,
            ),
            observation: row
                .last_observation
                .as_deref()
                .and_then(EggpoolProviderObservation::from_wire),
        });
    }
    Ok(EggpoolHealthSnapshot {
        schema_version: wire.schema_version,
        proxy,
        available: wire.proxy.available,
        reason_code: wire.proxy.reason_code.clone(),
        uptime_seconds: wire.proxy.uptime_seconds,
        model_count: wire.proxy.model_count,
        routable_accounts: wire.proxy.routable_accounts,
        enabled_accounts: wire.proxy.enabled_accounts,
        providers,
    })
}

/// Bound an optional identity or reason string before it can be rendered.
fn bounded_optional(value: Option<&str>, max_bytes: usize) -> bool {
    value.is_none_or(|value| !value.is_empty() && value.len() <= max_bytes)
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

fn classify_body_error(error: &EggfetchError) -> EggpoolFetchOutcome {
    if matches!(error, EggfetchError::DecodedBodyTooLarge) {
        return EggpoolFetchOutcome::BodyTooLarge;
    }
    if matches!(
        error,
        EggfetchError::Timeout { .. } | EggfetchError::TransportIoTimeout { .. }
    ) {
        // eggfetch enforces `Timeout.total` as one absolute wall-clock
        // deadline through response-body EOF, so a body-stage timeout
        // honors the configured whole-request deadline category.
        return EggpoolFetchOutcome::Timeout;
    }
    EggpoolFetchOutcome::NetworkError
}

fn classify_health_request_error(failure: &RequestFailure) -> EggpoolHealthFetchOutcome {
    if matches!(failure.error(), EggfetchError::DecodedBodyTooLarge) {
        return EggpoolHealthFetchOutcome::BodyTooLarge;
    }
    if failure.is_timeout() {
        return EggpoolHealthFetchOutcome::Timeout;
    }
    match failure.network_failure_kind() {
        Some(NetworkFailureKind::Dns) => EggpoolHealthFetchOutcome::DnsFailure,
        Some(NetworkFailureKind::ConnectionRefused) => EggpoolHealthFetchOutcome::ConnectionRefused,
        Some(NetworkFailureKind::Connect | _) | None => EggpoolHealthFetchOutcome::NetworkError,
    }
}

fn classify_health_body_error(error: &EggfetchError) -> EggpoolHealthFetchOutcome {
    if matches!(error, EggfetchError::DecodedBodyTooLarge) {
        return EggpoolHealthFetchOutcome::BodyTooLarge;
    }
    if matches!(
        error,
        EggfetchError::Timeout { .. } | EggfetchError::TransportIoTimeout { .. }
    ) {
        return EggpoolHealthFetchOutcome::Timeout;
    }
    EggpoolHealthFetchOutcome::NetworkError
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

/// Bounded depth of one worker's completed-result channel.
///
/// A pane that stops reading must apply backpressure here rather than grow the
/// daemon's memory, so the worker waits for a slot — inside a `select!` that
/// still observes cancellation.
const RESULT_CHANNEL_CAPACITY: usize = 4;

/// Start one worker for one configured `EggPool` endpoint.
///
/// The single request task reads the summary and service-health planes
/// concurrently, so neither plane can delay the other and both are aborted
/// together when the desired state supersedes them.
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
    let (result_tx, result_rx) = tokio::sync::mpsc::channel(RESULT_CHANNEL_CAPACITY);
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
                    let (generation, period, started_at, summary, health) = match completed {
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
                            EggpoolHealthFetchOutcome::NetworkError,
                        ),
                    };
                    let delivered = deliver_result_or_interrupt(
                        &result_tx,
                        &cancel,
                        &mut control_rx,
                        EggpoolResult {
                            generation,
                            period,
                            started_at,
                            completed_at: clock.now(),
                            summary,
                            health,
                        },
                    )
                    .await;
                    match delivered {
                        ResultDelivery::Delivered => {
                            if worker.desired.active {
                                // Two clocks: `started_at`/`completed_at` use
                                // wall-clock `now()` while the refresh deadline
                                // uses the Tokio timer clock `tokio_now()`. A
                                // fake clock must advance the wall clock for
                                // timestamps and rely on the runtime clock for
                                // deadlines (see `FakeClock`); advancing one
                                // without the other breaks refresh scheduling
                                // silently.
                                worker.next_refresh_at =
                                    Some(clock.tokio_now() + REFRESH_INTERVAL);
                            }
                        }
                        ResultDelivery::Cancelled
                        | ResultDelivery::ControlGone
                        | ResultDelivery::ResultReceiverGone => {
                            // Shutting down, or the daemon is gone: nothing is
                            // left to report to and no pane can be waiting.
                            worker.abort_request();
                            break;
                        }
                        ResultDelivery::Superseded(desired) => {
                            // A newer intent was published while this result was
                            // backpressured. Do not arm the old result's passive
                            // refresh deadline — that would schedule a fetch for
                            // superseded intent. Convergence owns the transition:
                            // it adopts the newest retained state, clears the
                            // passive deadline, and reports whether exactly one
                            // request must start for it.
                            if worker.converge(desired) {
                                worker.start_request(&client, &endpoint, &clock);
                            }
                        }
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

/// Deliver one completed result, yielding to shutdown **and to newer intent**
/// while it blocks.
///
/// The fetch has already finished, so a full result channel is ordinary
/// backpressure — the worker must wait for a slot rather than buffer without
/// bound. Awaiting `send` in the worker loop's branch body parked the whole
/// loop, leaving cancellation, a newer desired state, and the passive refresh
/// deadline unpolled until a slot happened to free. Selecting inside this
/// primitive keeps that wait bounded by the same signals as every other worker
/// branch, and keeps the completed result losslessly abandoned when the daemon
/// is shutting down.
///
/// Selecting only on cancellation was not enough. While the fifth result waits
/// for a slot the worker is *inside* this helper and no longer polling
/// `control_rx.changed()`, so a newly published desired state — deactivation, a
/// period change, a refresh generation — stayed unapplied until the daemon
/// drained a result slot. That is the Plan-151 contract that the worker converges
/// onto the *newest* state rather than replaying stale work, and full backpressure
/// is exactly when it matters most: leaving the `EggPool` pane could otherwise be
/// delayed behind a completed fetch nobody is reading. So the newest retained
/// desired state is inspected with `borrow_and_update()` and, when it supersedes
/// this result, the result is abandoned and that state is handed back to the
/// worker loop to converge on.
///
/// A notification carrying an *equivalent* state is consumed and the wait
/// continues: the completed result is still authoritative for it, and dropping
/// it would turn a redundant publication into a lost result.
///
/// The reservation is taken with `reserve()` rather than by sending directly so
/// a lost race returns the value instead of consuming it — the loop needs the
/// same result to retry with.
///
/// `biased` ordering makes shutdown win a race against a slot that frees at the
/// same instant: once this worker is ending, no pane can be waiting for the
/// result. The newest intent is authoritative for the same reason — an obsolete
/// completed result need not be delivered ahead of intent already known to be
/// newer.
///
/// Same shape the cron worker uses for its own observation hand-off.
async fn deliver_result_or_interrupt(
    sender: &tokio::sync::mpsc::Sender<EggpoolResult>,
    cancel: &tokio_util::sync::CancellationToken,
    control_rx: &mut tokio::sync::watch::Receiver<EggpoolDesiredState>,
    result: EggpoolResult,
) -> ResultDelivery {
    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => return ResultDelivery::Cancelled,
            changed = control_rx.changed() => {
                if changed.is_err() {
                    // No publisher remains, so no desired state can supersede
                    // this one. Cancel is not required here.
                    return ResultDelivery::ControlGone;
                }
                let desired = *control_rx.borrow_and_update();
                if !supersedes_result(&result, desired) {
                    // An equivalent publication. Consume it and keep waiting:
                    // the completed result is still the answer for this state.
                    continue;
                }
                return ResultDelivery::Superseded(desired);
            }
            reserved = sender.reserve() => match reserved {
                Ok(permit) => {
                    permit.send(result);
                    return ResultDelivery::Delivered;
                }
                // The daemon is gone: nothing is left to report to.
                Err(_) => return ResultDelivery::ResultReceiverGone,
            },
        }
    }
}

/// How one completed-result hand-off ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResultDelivery {
    /// The bounded channel accepted the result.
    Delivered,
    /// Shutdown pre-empted the hand-off; the result was abandoned.
    Cancelled,
    /// The control publisher is gone, so the worker ends.
    ControlGone,
    /// The result receiver is gone, so the worker ends.
    ResultReceiverGone,
    /// A newer desired state made this result obsolete.
    ///
    /// Carries that newest state so the worker converges onto it directly rather
    /// than waiting to be woken a second time.
    Superseded(EggpoolDesiredState),
}

/// Whether a newly published desired state makes a completed result obsolete.
///
/// Inactive always supersedes — the pane no longer wants `EggPool` work at all.
/// An active state supersedes when the rolling window or the refresh generation
/// differs, because the result was fetched for the previous one. An identical
/// active state does not: the result still answers it.
fn supersedes_result(result: &EggpoolResult, desired: EggpoolDesiredState) -> bool {
    !desired.active || desired.period != result.period || desired.generation != result.generation
}

/// One completed `EggPool` request carrying both independent planes.
type RequestTask = tokio::task::JoinHandle<(
    u64,
    EggpoolPeriod,
    Instant,
    EggpoolFetchOutcome,
    EggpoolHealthFetchOutcome,
)>;

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
        // The summary and health planes are read concurrently inside this
        // one request task, so a slow or unavailable status route never
        // delays a valid summary and vice versa. Both are aborted together
        // when this request is superseded.
        let (summary, health) = tokio::join!(
            client.fetch(&endpoint, period),
            client.fetch_health(&endpoint)
        );
        (generation, period, started_at, summary, health)
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
                        // Only summary paths are recorded: the health plane
                        // is asserted from the delivered result.
                        let is_summary = path.starts_with("/api/stats/summary");
                        let period = path.split("period=").nth(1).unwrap_or("1h").to_string();
                        if is_summary {
                            let _ = request_tx.send(path).await;
                        }
                        // Hold the response until the gate opens.
                        while !*gate_rx.borrow_and_update() {
                            if gate_rx.changed().await.is_err() {
                                return;
                            }
                        }
                        let body = if is_summary {
                            let ordinal =
                                ordinal.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                            format!(
                                "{{\"period\":\"{period}\",\"accounted_tokens\":{ordinal},\"cache_read_ratio\":null,\"tokens_per_second\":1.5,\"avg_ttft_ms\":12.0,\"streamed_requests\":0}}"
                            )
                        } else {
                            canonical_status_body("ready", "ready", "verified")
                        };
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                            body.len()
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

    /// A valid schema-version-1 status payload in `EggPool`'s own serialized
    /// shape.
    ///
    /// Upstream provenance for every passing status fixture in this module:
    ///
    /// ```text
    /// repo:          eggstack/eggpool
    /// commit:        43c987ea458bd563d5108fd8051ad31185704bb0
    /// type:          rust/src/operations/status.rs::ProxyStatusSnapshot
    /// nested types:  ProxyHealthSummary / ProviderHealthSummary / RuntimeHealthSummary
    /// serialization: serde JSON
    /// ```
    ///
    /// Field names and placement follow that type: account counts are nested
    /// under `proxy`, and provider rows carry `provider_id` /
    /// `last_observation`. The payload also carries the canonical fields Gregg
    /// does not display (`observed_at`, `runtime`, `proxy.ready`,
    /// `proxy.version`, `proxy.base_url`, provider account/probe details, and
    /// the provider reason code), which must decode as ignored. This is a
    /// structurally canonical synthetic fixture, not a byte-for-byte capture
    /// from a live `EggPool` response.
    /// A payload that does not match this shape is not a passing schema, so
    /// this constructor is the single source of the status matrix instead of
    /// a locally invented variant.
    fn canonical_status_body(proxy: &str, provider: &str, observation: &str) -> String {
        format!(
            "{{\"schema_version\":1,\"observed_at\":\"2026-10-02T12:00:00Z\",\"runtime\":{{\"generation\":17,\"digest_prefix\":\"0123456789ab\",\"reload\":\"idle\",\"tasks\":\"4/4\",\"db\":\"ok\",\"retiring\":0}},\"proxy\":{{\"status\":\"{proxy}\",\"available\":true,\"ready\":true,\"version\":\"0.9.1\",\"base_url\":\"http://127.0.0.1:11300\",\"uptime_seconds\":42.5,\"model_count\":3,\"routable_accounts\":5,\"enabled_accounts\":6}},\"providers\":[{{\"provider_id\":\"openai\",\"status\":\"{provider}\",\"last_observation\":\"{observation}\",\"enabled_accounts\":2,\"total_accounts\":2,\"routable_accounts\":1,\"backoff_accounts\":1,\"unavailable_accounts\":1,\"model_count\":3,\"last_probe_age_seconds\":2,\"last_probe_latency_ms\":31,\"last_probe_status_code\":200,\"reason_code\":\"slow\"}}]}}"
        )
    }

    /// One canned response for a read-only `EggPool` route.
    #[derive(Clone)]
    struct RouteReply {
        status: String,
        body: String,
    }

    impl RouteReply {
        fn ok(body: String) -> Self {
            Self {
                status: "200 OK".to_owned(),
                body,
            }
        }

        fn status(status: &str) -> Self {
            Self {
                status: status.to_owned(),
                body: String::new(),
            }
        }
    }

    /// A loopback `EggPool` that answers both read-only routes with
    /// caller-supplied replies, so the two planes can fail independently.
    async fn server_routes(
        summary: RouteReply,
        status: RouteReply,
    ) -> (u16, mpsc::Receiver<String>, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (request_tx, request_rx) = mpsc::channel(32);
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let request_tx = request_tx.clone();
                let summary = summary.clone();
                let status = status.clone();
                tokio::spawn(async move {
                    let mut buffered = vec![0; 8192];
                    let mut used = 0;
                    loop {
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
                        let _ = request_tx.send(path.clone()).await;
                        let reply = if path.starts_with("/api/stats/summary") {
                            &summary
                        } else {
                            &status
                        };
                        let response = format!(
                            "HTTP/1.1 {}\r\nContent-Length: {}\r\n\r\n{}",
                            reply.status,
                            reply.body.len(),
                            reply.body
                        );
                        if stream.write_all(response.as_bytes()).await.is_err() {
                            return;
                        }
                        used = 0;
                    }
                });
            }
        });
        (port, request_rx, task)
    }

    fn summary_body(period: &str) -> String {
        format!(
            r#"{{"period":"{period}","accounted_tokens":42,"cache_read_ratio":0.25,"tokens_per_second":1.5,"avg_ttft_ms":12.0,"streamed_requests":3}}"#
        )
    }

    async fn fetch_both(
        port: u16,
        api_key_env: Option<&str>,
    ) -> (EggpoolFetchOutcome, EggpoolHealthFetchOutcome) {
        let client = EggpoolClient::new(Duration::from_secs(2));
        let endpoint = endpoint(port, api_key_env);
        tokio::join!(
            client.fetch(&endpoint, EggpoolPeriod::Hour),
            client.fetch_health(&endpoint)
        )
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
        let mut app = crate::state::AppState::synthetic(&app_config(port));
        app.begin_eggpool_request(true);
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
        assert_eq!(result.summary, EggpoolFetchOutcome::NetworkError);
        assert_eq!(result.health, EggpoolHealthFetchOutcome::NetworkError);
        assert_eq!((result.generation, result.period), (1, EggpoolPeriod::Hour));
        cancel.cancel();
    }

    /// Build a completed result with no network involved.
    fn buffered_result(generation: u64) -> EggpoolResult {
        let at = Instant::now();
        EggpoolResult {
            generation,
            period: EggpoolPeriod::Hour,
            started_at: at,
            completed_at: at,
            summary: EggpoolFetchOutcome::NetworkError,
            health: EggpoolHealthFetchOutcome::NetworkError,
        }
    }

    /// Cancellation must win over a full result channel.
    ///
    /// The delivery of a finished fetch is ordinary backpressure when the pane
    /// is not reading. Awaiting that send in the completion branch body parked
    /// the whole loop: cancellation, a newer desired state, and the passive
    /// refresh deadline were all left unpolled until a slot happened to free.
    ///
    /// This exercises the production delivery primitive against a channel this
    /// test fills itself. Synthesizing backpressure from real network failures
    /// (closed port, local server) made the precondition — *full channel* —
    /// depend on OS connection timing, which is why this used to fail on the
    /// Windows runner before asserting anything about cancellation.
    #[tokio::test]
    async fn a_full_result_channel_never_delays_cancellation() {
        let (result_tx, mut result_rx) = mpsc::channel(RESULT_CHANNEL_CAPACITY);
        let cancel = tokio_util::sync::CancellationToken::new();
        let (_control_tx, mut control_rx) =
            tokio::sync::watch::channel(EggpoolDesiredState::INACTIVE);

        // Precondition: exactly the bounded capacity is buffered, in order.
        for generation in 1..=RESULT_CHANNEL_CAPACITY as u64 {
            result_tx
                .send(buffered_result(generation))
                .await
                .expect("a slot is free while filling to capacity");
        }
        assert_eq!(result_rx.len(), RESULT_CHANNEL_CAPACITY);

        // The fifth delivery is blocked, and the worker is still live.
        {
            let mut blocked = Box::pin(deliver_result_or_interrupt(
                &result_tx,
                &cancel,
                &mut control_rx,
                buffered_result(99),
            ));
            tokio::select! {
                biased;
                delivered = &mut blocked => panic!(
                    "a full channel must block delivery, but it reported {delivered:?}"
                ),
                () = tokio::task::yield_now() => {}
            }

            // Cancellation must release the blocked delivery without the
            // receiver ever freeing capacity. A `yield_now` bound rather than a
            // timer: a live token leaves the delivery genuinely pending, so
            // this fails fast (instead of hanging) if delivery ever stops
            // observing cancellation.
            cancel.cancel();
            let delivered = tokio::select! {
                biased;
                delivered = blocked.as_mut() => delivered,
                () = tokio::task::yield_now() => {
                    panic!("cancellation must complete the blocked delivery")
                }
            };
            assert_eq!(
                delivered,
                ResultDelivery::Cancelled,
                "a cancelled delivery reports the result as abandoned"
            );
        }

        // The abandoned result never entered the channel, and the buffered
        // four are intact and in order.
        assert_eq!(result_rx.len(), RESULT_CHANNEL_CAPACITY);
        for expected in 1..=RESULT_CHANNEL_CAPACITY as u64 {
            let received = result_rx.recv().await.expect("a buffered result");
            assert_eq!(received.generation, expected);
        }

        // Dropping the last sender closes the receiver: no producer survives.
        drop(result_tx);
        assert!(
            result_rx.recv().await.is_none(),
            "the closed channel proves the sender is gone"
        );
    }

    /// A full result channel must not delay **deactivation**.
    ///
    /// This is the defect Plan 176 left open: cancellation was already first in
    /// the select, but the control watch was not polled at all while delivery
    /// was blocked. So closing the `EggPool` pane could leave the worker still
    /// trying to hand over a completed result — deactivation stuck behind
    /// receiver capacity.
    ///
    /// The full-channel precondition is built by filling the real bounded
    /// channel, as Plan 176 established: a closed port or OS connection timing
    /// proves nothing about the hand-off.
    #[tokio::test]
    async fn a_full_result_channel_never_delays_deactivation() {
        let (result_tx, result_rx) = mpsc::channel(RESULT_CHANNEL_CAPACITY);
        let cancel = tokio_util::sync::CancellationToken::new();
        let (control_tx, mut control_rx) =
            tokio::sync::watch::channel(EggpoolDesiredState::INACTIVE);
        let control = EggpoolControl { sender: control_tx };
        for generation in 1..=RESULT_CHANNEL_CAPACITY as u64 {
            result_tx
                .send(buffered_result(generation))
                .await
                .expect("a slot is free while filling to capacity");
        }

        let mut blocked = Box::pin(deliver_result_or_interrupt(
            &result_tx,
            &cancel,
            &mut control_rx,
            buffered_result(7),
        ));
        tokio::select! {
            biased;
            delivered = &mut blocked => panic!("a full channel must block delivery, got {delivered:?}"),
            () = tokio::task::yield_now() => {}
        }

        // No receiver capacity is freed; only the deactivation is published.
        control
            .publish(inactive(EggpoolPeriod::Hour, 7))
            .expect("a live publisher");
        let delivered = tokio::select! {
            biased;
            delivered = blocked.as_mut() => delivered,
            () = tokio::task::yield_now() => {
                panic!("a deactivation must complete the blocked delivery")
            }
        };
        assert_eq!(
            delivered,
            ResultDelivery::Superseded(inactive(EggpoolPeriod::Hour, 7)),
            "the inactive newest state is handed back for the worker to converge on"
        );
        assert_eq!(
            result_rx.len(),
            RESULT_CHANNEL_CAPACITY,
            "the abandoned result must not occupy a slot"
        );
    }

    /// A full result channel must not delay adoption of a newer period or
    /// generation, and only the **newest** retained state is adopted.
    ///
    /// Watch latest-value coalescing is the product contract, so several
    /// publications while blocked must collapse to the last one with no
    /// intermediate replay required.
    #[tokio::test]
    async fn a_full_result_channel_never_delays_a_newer_period_or_generation() {
        let (result_tx, result_rx) = mpsc::channel(RESULT_CHANNEL_CAPACITY);
        let cancel = tokio_util::sync::CancellationToken::new();
        let (control_tx, mut control_rx) =
            tokio::sync::watch::channel(EggpoolDesiredState::INACTIVE);
        let control = EggpoolControl { sender: control_tx };
        for generation in 1..=RESULT_CHANNEL_CAPACITY as u64 {
            result_tx
                .send(buffered_result(generation))
                .await
                .expect("a slot is free while filling to capacity");
        }

        let mut blocked = Box::pin(deliver_result_or_interrupt(
            &result_tx,
            &cancel,
            &mut control_rx,
            buffered_result(1),
        ));
        tokio::select! {
            biased;
            delivered = &mut blocked => panic!("a full channel must block delivery, got {delivered:?}"),
            () = tokio::task::yield_now() => {}
        }

        // Three publications while blocked; only the last is authoritative.
        control.publish(desired(EggpoolPeriod::Month, 1)).unwrap();
        control.publish(desired(EggpoolPeriod::Hour, 2)).unwrap();
        let newest = desired(EggpoolPeriod::Day, 3);
        control.publish(newest).unwrap();

        let delivered = tokio::select! {
            biased;
            delivered = blocked.as_mut() => delivered,
            () = tokio::task::yield_now() => {
                panic!("a newer period/generation must complete the blocked delivery")
            }
        };
        assert_eq!(
            delivered,
            ResultDelivery::Superseded(newest),
            "only the newest retained desired state is adopted"
        );
        assert_eq!(result_rx.len(), RESULT_CHANNEL_CAPACITY);
    }

    /// An **equivalent** publication must not discard a still-current result.
    ///
    /// The pane republishes the same `(active, period, generation)` on ordinary
    /// repaints. Treating that as supersession would turn a redundant
    /// publication into a silently dropped `EggPool` result.
    #[tokio::test]
    async fn an_equivalent_desired_state_does_not_discard_a_valid_result() {
        let (result_tx, mut result_rx) = mpsc::channel(RESULT_CHANNEL_CAPACITY);
        let cancel = tokio_util::sync::CancellationToken::new();
        let (control_tx, mut control_rx) =
            tokio::sync::watch::channel(EggpoolDesiredState::INACTIVE);
        let control = EggpoolControl { sender: control_tx };
        for generation in 1..=RESULT_CHANNEL_CAPACITY as u64 {
            result_tx
                .send(buffered_result(generation))
                .await
                .expect("a slot is free while filling to capacity");
        }

        let mut blocked = Box::pin(deliver_result_or_interrupt(
            &result_tx,
            &cancel,
            &mut control_rx,
            buffered_result(7),
        ));
        tokio::select! {
            biased;
            delivered = &mut blocked => panic!("a full channel must block delivery, got {delivered:?}"),
            () = tokio::task::yield_now() => {}
        }

        // The same state the completed result answers.
        control.publish(desired(EggpoolPeriod::Hour, 7)).unwrap();
        // Give the helper every chance to react to that notification.
        for _ in 0..64 {
            tokio::select! {
                biased;
                delivered = &mut blocked => panic!(
                    "an equivalent publication must not abandon the result, got {delivered:?}"
                ),
                () = tokio::task::yield_now() => {}
            }
        }

        // Free exactly one slot: the result must now be delivered. The bounded
        // mpsc is FIFO, so the newcomer appends behind the buffered placeholders.
        result_rx.recv().await.expect("a buffered placeholder");
        assert_eq!(
            blocked.await,
            ResultDelivery::Delivered,
            "an equivalent notification is consumed, then the result is delivered"
        );
        assert_eq!(
            result_rx.len(),
            RESULT_CHANNEL_CAPACITY,
            "delivery refilled the slot it took"
        );
        for expected in 2..=RESULT_CHANNEL_CAPACITY as u64 {
            assert_eq!(
                result_rx
                    .recv()
                    .await
                    .expect("a buffered placeholder")
                    .generation,
                expected,
                "the remaining placeholders stay intact and in order"
            );
        }
        assert_eq!(
            result_rx
                .recv()
                .await
                .expect("the delivered result")
                .generation,
            7,
            "the result must be delivered, not dropped by the equivalent publication"
        );
    }

    /// Cancellation still outranks a superseding control change.
    #[tokio::test]
    async fn cancellation_outranks_a_superseding_control_change() {
        let (result_tx, result_rx) = mpsc::channel(RESULT_CHANNEL_CAPACITY);
        let cancel = tokio_util::sync::CancellationToken::new();
        let (control_tx, mut control_rx) =
            tokio::sync::watch::channel(EggpoolDesiredState::INACTIVE);
        let control = EggpoolControl { sender: control_tx };
        for generation in 1..=RESULT_CHANNEL_CAPACITY as u64 {
            result_tx
                .send(buffered_result(generation))
                .await
                .expect("a slot is free while filling to capacity");
        }

        let mut blocked = Box::pin(deliver_result_or_interrupt(
            &result_tx,
            &cancel,
            &mut control_rx,
            buffered_result(7),
        ));
        tokio::select! {
            biased;
            delivered = &mut blocked => panic!("a full channel must block delivery, got {delivered:?}"),
            () = tokio::task::yield_now() => {}
        }

        control.publish(inactive(EggpoolPeriod::Hour, 8)).unwrap();
        cancel.cancel();
        assert_eq!(
            blocked.await,
            ResultDelivery::Cancelled,
            "shutdown is the higher-priority signal"
        );
        assert_eq!(result_rx.len(), RESULT_CHANNEL_CAPACITY);
    }

    /// A gone control publisher or result receiver ends delivery rather than
    /// parking forever.
    #[tokio::test]
    async fn a_gone_channel_ends_delivery() {
        let (result_tx, result_rx) = mpsc::channel(RESULT_CHANNEL_CAPACITY);
        let cancel = tokio_util::sync::CancellationToken::new();
        let (control_tx, mut control_rx) =
            tokio::sync::watch::channel(EggpoolDesiredState::INACTIVE);

        // No publisher: the newest state can never supersede this one.
        drop(control_tx);
        assert_eq!(
            deliver_result_or_interrupt(&result_tx, &cancel, &mut control_rx, buffered_result(1),)
                .await,
            ResultDelivery::ControlGone
        );

        // No receiver: the daemon is gone.
        let (control_tx, mut control_rx) =
            tokio::sync::watch::channel(EggpoolDesiredState::INACTIVE);
        let control = EggpoolControl { sender: control_tx };
        control.publish(desired(EggpoolPeriod::Hour, 1)).unwrap();
        drop(result_rx);
        assert_eq!(
            deliver_result_or_interrupt(&result_tx, &cancel, &mut control_rx, buffered_result(1),)
                .await,
            ResultDelivery::ResultReceiverGone
        );
    }

    /// Worker level: a newer desired state starts its request while the prior
    /// completed result is still blocked by full output backpressure.
    ///
    /// The primitive is pinned by the helper tests above; this proves the worker
    /// loop converges on what the primitive hands back. Nothing reads
    /// `worker.results` here, so each passive refresh fills one slot of the
    /// bounded channel and the next completed result is genuinely undeliverable
    /// — the exact pressure that used to strand deactivation and period changes.
    #[tokio::test(start_paused = true)]
    async fn a_worker_converges_onto_newer_intent_while_a_result_is_backpressured() {
        let (port, mut requests, gate, server_task) = server_gated().await;
        let cancel = tokio_util::sync::CancellationToken::new();
        let worker = worker_for(port, &cancel);
        worker
            .control
            .publish(desired(EggpoolPeriod::Hour, 1))
            .unwrap();
        assert_eq!(
            next_request(&mut requests).await,
            "/api/stats/summary?period=1h"
        );
        // Open the gate once: it then answers every request, so each cycle below
        // completes.
        gate.send(true).ok();

        // One passive refresh per cycle. The first three fill the remaining
        // slots; the fourth's result cannot be delivered.
        for cycle in 1..=RESULT_CHANNEL_CAPACITY {
            tokio::time::advance(REFRESH_INTERVAL).await;
            assert_eq!(
                next_request(&mut requests).await,
                "/api/stats/summary?period=1h",
                "passive cycle {cycle} must issue the refresh"
            );
            for _ in 0..256 {
                tokio::task::yield_now().await;
            }
        }
        assert_eq!(
            worker.results.len(),
            RESULT_CHANNEL_CAPACITY,
            "the channel is full and no later result may be delivered into it"
        );

        // A newer intent must still be adopted, and its request started, with
        // the previous result undeliverable.
        worker
            .control
            .publish(desired(EggpoolPeriod::Month, 2))
            .unwrap();
        assert_eq!(
            next_request(&mut requests).await,
            "/api/stats/summary?period=30d",
            "convergence must not wait for a result slot nobody is draining"
        );

        // The abandoned result left the channel untouched, and deactivation is
        // honoured without one either.
        worker
            .control
            .publish(inactive(EggpoolPeriod::Month, 3))
            .unwrap();
        for _ in 0..1_000 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            worker.results.len(),
            RESULT_CHANNEL_CAPACITY,
            "no further result was queued behind the backpressure"
        );
        assert_eq!(
            worker.control.published(),
            inactive(EggpoolPeriod::Month, 3)
        );

        cancel.cancel();
        server_task.abort();
        let _ = server_task.await;
    }

    #[tokio::test(start_paused = true)]
    async fn worker_cancellation_ends_the_worker_and_closes_its_result_stream() {
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

        cancel.cancel();
        let after_cancel =
            tokio::time::timeout(Duration::from_secs(5), worker.results.recv()).await;
        assert!(
            after_cancel.is_ok(),
            "a cancelled worker must end rather than park"
        );
        assert!(
            after_cancel.expect("bounded").is_none(),
            "the worker ends, which closes the channel"
        );
        stop(&mut worker, &cancel, server_task).await;
        gate.send(true).ok();
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
        assert!(matches!(result.summary, EggpoolFetchOutcome::Online(_)));
        assert!(matches!(
            result.health,
            EggpoolHealthFetchOutcome::Online(_)
        ));
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
        assert!(matches!(result.summary, EggpoolFetchOutcome::Online(_)));
        assert!(matches!(
            result.health,
            EggpoolHealthFetchOutcome::Online(_)
        ));
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
            final_result.summary,
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
    async fn worker_delivers_both_planes_and_partial_success() {
        // The gated server answers the summary route normally and returns a
        // degraded health snapshot, so one result carries both planes.
        let (port, mut requests, gate, server_task) = server_gated().await;
        gate.send(true).ok();
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
        let result = next_result(&mut worker).await;
        assert!(matches!(result.summary, EggpoolFetchOutcome::Online(_)));
        let EggpoolHealthFetchOutcome::Online(snapshot) = result.health else {
            panic!("the health plane is delivered with the summary plane");
        };
        assert_eq!(snapshot.proxy, EggpoolProxyHealth::Ready);
        assert_eq!(snapshot.providers.len(), 1);
        stop(&mut worker, &cancel, server_task).await;
    }

    #[tokio::test]
    async fn a_stalled_health_route_does_not_hide_a_summary_failure() {
        // A status route that never answers must not be confused with a
        // `EggPool`-reported proxy status, and the summary plane keeps its
        // own outcome.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server_task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let mut buffered = vec![0; 8192];
                    let mut used = 0;
                    loop {
                        if find_header_end(&buffered[..used]).is_some() {
                            let head = String::from_utf8_lossy(&buffered[..used]).into_owned();
                            let path = head
                                .lines()
                                .next()
                                .unwrap_or_default()
                                .split_whitespace()
                                .nth(1)
                                .unwrap_or_default()
                                .to_string();
                            if path.starts_with("/api/stats/summary") {
                                let body = summary_body("1h");
                                let response = format!(
                                    "HTTP/1.1 503 Service Unavailable\r\nContent-Length: {}\r\n\r\n{body}",
                                    body.len()
                                );
                                if stream.write_all(response.as_bytes()).await.is_err() {
                                    return;
                                }
                            } else {
                                // Never answer the status route.
                                std::future::pending::<()>().await;
                            }
                            used = 0;
                        } else {
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
                        }
                    }
                });
            }
        });

        let client = EggpoolClient::new(Duration::from_millis(200));
        let endpoint = endpoint(port, None);
        let (summary, health) = tokio::join!(
            client.fetch(&endpoint, EggpoolPeriod::Hour),
            client.fetch_health(&endpoint)
        );
        assert_eq!(summary, EggpoolFetchOutcome::HttpStatus(503));
        // A transport failure is a local fact, never a reported proxy state.
        assert_eq!(health, EggpoolHealthFetchOutcome::Timeout);
        server_task.abort();
        let _ = server_task.await;
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
        let app = crate::state::AppState::synthetic(&crate::config::Config::default());
        assert!(app.eggpool.is_none());
        assert!(app.eggpool_desired_state().is_none());
    }

    #[tokio::test]
    async fn health_decodes_ready_degraded_and_unready_proxy_states() {
        for (proxy, expected) in [
            ("ready", EggpoolProxyHealth::Ready),
            ("degraded", EggpoolProxyHealth::Degraded),
            ("unready", EggpoolProxyHealth::Unready),
        ] {
            let (port, _requests, server) = server_routes(
                RouteReply::ok(summary_body("1h")),
                RouteReply::ok(canonical_status_body(proxy, "ready", "verified")),
            )
            .await;
            let (summary, health) = fetch_both(port, None).await;
            assert!(matches!(summary, EggpoolFetchOutcome::Online(_)));
            let EggpoolHealthFetchOutcome::Online(snapshot) = health else {
                panic!("expected a health snapshot for proxy {proxy}");
            };
            assert_eq!(snapshot.schema_version, 1);
            assert_eq!(snapshot.proxy, expected);
            assert!(snapshot.available);
            assert_eq!(snapshot.uptime_seconds, Some(42.5));
            assert_eq!(snapshot.model_count, Some(3));
            assert_eq!(snapshot.routable_accounts, Some(5));
            assert_eq!(snapshot.enabled_accounts, Some(6));
            server.abort();
        }
    }

    #[tokio::test]
    async fn health_decodes_every_provider_state_without_conflation() {
        let body =
            r#"{"schema_version":1,"proxy":{"status":"degraded","available":true},"providers":[
            {"provider_id":"a","status":"ready","last_observation":"verified"},
            {"provider_id":"b","status":"degraded","last_observation":"failed"},
            {"provider_id":"c","status":"unavailable","last_observation":"stale"},
            {"provider_id":"d","status":"disabled","last_observation":"never"},
            {"provider_id":"e","status":"something-new","last_observation":"also-new"},
            {"provider_id":"f"}
        ]}"#
            .replace('\n', "");
        let (port, _requests, server) =
            server_routes(RouteReply::ok(summary_body("1h")), RouteReply::ok(body)).await;
        let (_summary, health) = fetch_both(port, None).await;
        let EggpoolHealthFetchOutcome::Online(snapshot) = health else {
            panic!("expected a health snapshot");
        };
        let statuses: Vec<EggpoolProviderHealth> =
            snapshot.providers.iter().map(|row| row.status).collect();
        assert_eq!(
            statuses,
            [
                EggpoolProviderHealth::Ready,
                EggpoolProviderHealth::Degraded,
                EggpoolProviderHealth::Unavailable,
                EggpoolProviderHealth::Disabled,
                // An unrecognized value is genuinely unknown, not a guess.
                EggpoolProviderHealth::Unknown,
                EggpoolProviderHealth::Unknown,
            ]
        );
        let observations: Vec<Option<EggpoolProviderObservation>> = snapshot
            .providers
            .iter()
            .map(|row| row.observation)
            .collect();
        assert_eq!(
            observations,
            [
                Some(EggpoolProviderObservation::Verified),
                Some(EggpoolProviderObservation::Failed),
                Some(EggpoolProviderObservation::Stale),
                Some(EggpoolProviderObservation::Never),
                None,
                None,
            ]
        );
        assert_eq!(
            snapshot.provider_counts(),
            [1, 1, 1, 1, 2],
            "bounded provider counts follow the decoded rows"
        );
        server.abort();
    }

    #[tokio::test]
    async fn public_summary_with_authenticated_health_stays_usable() {
        // EggPool keeps `/api/status` authenticated even when the dashboard
        // is public. No key is configured, so the status read is
        // unauthenticated and answered with 401.
        let (port, mut requests, server) = server_routes(
            RouteReply::ok(summary_body("1h")),
            RouteReply::status("401 Unauthorized"),
        )
        .await;
        // No key is configured, so both reads are unauthenticated: the
        // public summary still succeeds and only the status route is refused.
        let client = EggpoolClient::new(Duration::from_secs(2));
        let endpoint = endpoint(port, None);
        let (summary, health) = tokio::join!(
            client.fetch(&endpoint, EggpoolPeriod::Hour),
            client.fetch_health(&endpoint)
        );
        assert!(matches!(summary, EggpoolFetchOutcome::Online(_)));
        assert_eq!(health, EggpoolHealthFetchOutcome::AuthenticationRequired);
        let seen: Vec<String> = std::iter::from_fn(|| requests.try_recv().ok()).collect();
        assert!(seen.contains(&"/api/stats/summary?period=1h".to_owned()));
        assert!(seen.contains(&"/api/status".to_owned()));
        server.abort();
    }

    #[tokio::test]
    async fn older_eggpool_status_404_is_unsupported_not_a_summary_failure() {
        let (port, _requests, server) = server_routes(
            RouteReply::ok(summary_body("1h")),
            RouteReply::status("404 Not Found"),
        )
        .await;
        let (summary, health) = fetch_both(port, None).await;
        assert!(matches!(summary, EggpoolFetchOutcome::Online(_)));
        assert_eq!(health, EggpoolHealthFetchOutcome::Unsupported);
        server.abort();
    }

    #[tokio::test]
    async fn disabled_dashboard_keeps_valid_health_visible() {
        let (port, _requests, server) = server_routes(
            RouteReply::status("404 Not Found"),
            RouteReply::ok(canonical_status_body("ready", "ready", "verified")),
        )
        .await;
        let (summary, health) = fetch_both(port, None).await;
        assert_eq!(summary, EggpoolFetchOutcome::StatsUnavailable);
        let EggpoolHealthFetchOutcome::Online(snapshot) = health else {
            panic!("health stays available when the dashboard is disabled");
        };
        assert_eq!(snapshot.proxy, EggpoolProxyHealth::Ready);
        server.abort();
    }

    #[tokio::test]
    async fn status_statuses_and_body_limit_are_bounded_separately() {
        for (status, expected) in [
            ("403 Forbidden", EggpoolHealthFetchOutcome::Forbidden),
            (
                "503 Service Unavailable",
                EggpoolHealthFetchOutcome::HttpStatus(503),
            ),
        ] {
            let (port, _requests, server) = server_routes(
                RouteReply::ok(summary_body("1h")),
                RouteReply::status(status),
            )
            .await;
            let (_summary, health) = fetch_both(port, None).await;
            assert_eq!(health, expected);
            server.abort();
        }
        let oversized = "x".repeat(MAX_STATUS_RESPONSE_BYTES + 1);
        let (port, _requests, server) = server_routes(
            RouteReply::ok(summary_body("1h")),
            RouteReply::ok(oversized),
        )
        .await;
        let (_summary, health) = fetch_both(port, None).await;
        assert_eq!(health, EggpoolHealthFetchOutcome::BodyTooLarge);
        server.abort();
    }

    #[tokio::test]
    async fn status_ceiling_is_per_route_and_does_not_widen_the_summary() {
        // A status payload larger than the summary bound still decodes,
        // proving the per-request ceiling rather than one global limit.
        let padding = "x".repeat(MAX_RESPONSE_BYTES * 2);
        let body = canonical_status_body("ready", "ready", "verified").replace(
            "\"schema_version\":1",
            &format!("\"schema_version\":1,\"note\":\"{padding}\""),
        );
        let (port, _requests, server) =
            server_routes(RouteReply::ok(summary_body("1h")), RouteReply::ok(body)).await;
        let (summary, health) = fetch_both(port, None).await;
        assert!(matches!(summary, EggpoolFetchOutcome::Online(_)));
        assert!(matches!(health, EggpoolHealthFetchOutcome::Online(_)));
        server.abort();

        // The summary route keeps its own 16 KiB ceiling.
        let big_summary = format!(
            "{{\"period\":\"1h\",\"pad\":\"{}\"}}",
            "x".repeat(MAX_RESPONSE_BYTES + 1)
        );
        let (port, _requests, server) = server_routes(
            RouteReply::ok(big_summary),
            RouteReply::ok(canonical_status_body("ready", "ready", "verified")),
        )
        .await;
        let (summary, health) = fetch_both(port, None).await;
        assert_eq!(summary, EggpoolFetchOutcome::BodyTooLarge);
        assert!(matches!(health, EggpoolHealthFetchOutcome::Online(_)));
        server.abort();
    }

    #[tokio::test]
    async fn malformed_status_never_invalidates_a_good_summary() {
        let (port, _requests, server) = server_routes(
            RouteReply::ok(summary_body("1h")),
            RouteReply::ok("not json at all".to_owned()),
        )
        .await;
        let (summary, health) = fetch_both(port, None).await;
        assert!(matches!(summary, EggpoolFetchOutcome::Online(_)));
        assert_eq!(health, EggpoolHealthFetchOutcome::DecodeError);
        server.abort();
    }

    #[tokio::test]
    async fn malformed_summary_never_invalidates_a_good_health_snapshot() {
        let (port, _requests, server) = server_routes(
            RouteReply::ok("{\"period\":".to_owned()),
            RouteReply::ok(canonical_status_body("degraded", "degraded", "failed")),
        )
        .await;
        let (summary, health) = fetch_both(port, None).await;
        assert_eq!(summary, EggpoolFetchOutcome::DecodeError);
        assert!(matches!(health, EggpoolHealthFetchOutcome::Online(_)));
        server.abort();
    }

    #[tokio::test]
    async fn unknown_schema_and_invalid_status_are_explicit_and_nonfatal() {
        let future = canonical_status_body("ready", "ready", "verified")
            .replace("\"schema_version\":1", "\"schema_version\":2");
        let (port, _requests, server) =
            server_routes(RouteReply::ok(summary_body("1h")), RouteReply::ok(future)).await;
        let (summary, health) = fetch_both(port, None).await;
        assert!(matches!(summary, EggpoolFetchOutcome::Online(_)));
        assert_eq!(health, EggpoolHealthFetchOutcome::UnsupportedSchema);
        server.abort();

        let unknown_proxy = canonical_status_body("sideways", "ready", "verified");
        let (port, _requests, server) = server_routes(
            RouteReply::ok(summary_body("1h")),
            RouteReply::ok(unknown_proxy),
        )
        .await;
        let (summary, health) = fetch_both(port, None).await;
        assert!(matches!(summary, EggpoolFetchOutcome::Online(_)));
        assert_eq!(health, EggpoolHealthFetchOutcome::InvalidStatus);
        server.abort();
    }

    /// A canonical status payload with `rows` provider rows whose
    /// `provider_id` is `p` plus the row index, used for the exact
    /// provider-row boundary.
    fn canonical_status_body_with_rows(rows: usize) -> String {
        let providers: Vec<String> = (0..rows)
            .map(|index| format!(r#"{{"provider_id":"p{index}","status":"ready"}}"#))
            .collect();
        format!(
            "{{\"schema_version\":1,\"proxy\":{{\"status\":\"ready\",\"available\":true}},\"providers\":[{}]}}",
            providers.join(",")
        )
    }

    /// A canonical status payload with one provider row of exactly `id_bytes`
    /// ASCII bytes, used for the exact provider-identity boundary.
    fn canonical_status_body_with_id(id_bytes: usize) -> String {
        let long_id = "i".repeat(id_bytes);
        format!(
            "{{\"schema_version\":1,\"proxy\":{{\"status\":\"ready\",\"available\":true}},\"providers\":[{{\"provider_id\":\"{long_id}\",\"status\":\"ready\"}}]}}"
        )
    }

    /// A canonical status payload whose proxy reason code is exactly
    /// `reason_bytes` ASCII bytes, used for the exact reason-code boundary.
    fn canonical_status_body_with_reason(reason_bytes: usize) -> String {
        let reason = "r".repeat(reason_bytes);
        format!(
            "{{\"schema_version\":1,\"proxy\":{{\"status\":\"unready\",\"available\":false,\"reason_code\":\"{reason}\"}}}}"
        )
    }

    /// Read one canonical status body and return the health outcome.
    async fn health_of(body: String) -> EggpoolHealthFetchOutcome {
        let (port, _requests, server) =
            server_routes(RouteReply::ok(summary_body("1h")), RouteReply::ok(body)).await;
        let health = fetch_both(port, None).await.1;
        server.abort();
        health
    }

    /// Read one canonical status body and report whether it was accepted.
    async fn health_accepts(body: String) -> bool {
        matches!(health_of(body).await, EggpoolHealthFetchOutcome::Online(_))
    }

    #[tokio::test]
    async fn bounded_status_contract_matches_the_producer_bounds_exactly() {
        // The limits are EggPool's own producer constants, and the fixtures
        // use literal byte/row counts so a drifted bound fails here instead of
        // quietly moving the expectation with the constant.
        assert_eq!(MAX_PROVIDER_ID_BYTES, 96);
        assert_eq!(MAX_STATUS_REASON_BYTES, 64);
        assert_eq!(MAX_STATUS_PROVIDER_ROWS, 256);

        // Each bound is exact, not conservative: the limit decodes and one
        // byte/row more is rejected.
        for (id_bytes, accepted) in [(96, true), (97, false)] {
            assert_eq!(
                health_accepts(canonical_status_body_with_id(id_bytes)).await,
                accepted,
                "a 96-byte provider ID decodes and 97 bytes is rejected"
            );
        }

        for (reason_bytes, accepted) in [(64, true), (65, false)] {
            assert_eq!(
                health_accepts(canonical_status_body_with_reason(reason_bytes)).await,
                accepted,
                "a 64-byte reason code decodes and 65 bytes is rejected"
            );
        }

        for (rows, accepted) in [(256, true), (257, false)] {
            assert_eq!(
                health_accepts(canonical_status_body_with_rows(rows)).await,
                accepted,
                "256 provider rows decode and 257 rows are rejected"
            );
        }

        // An out-of-contract number is rejected the same way, independent of
        // the three length bounds.
        let negative_uptime =
            r#"{"schema_version":1,"proxy":{"status":"ready","available":true,"uptime_seconds":-1.0}}"#
                .to_owned();
        assert_eq!(
            health_of(negative_uptime).await,
            EggpoolHealthFetchOutcome::InvalidStatus,
            "negative uptime is not a plausible EggPool report"
        );
    }

    #[tokio::test]
    async fn canonical_upstream_status_snapshot_decodes_and_ignores_unmodeled_fields() {
        // Provenance: eggstack/eggpool 43c987ea458bd563d5108fd8051ad31185704bb0,
        // rust/src/operations/status.rs::ProxyStatusSnapshot and its nested
        // health summary types, serialized as serde JSON. This
        // payload carries the canonical fields Gregg does not model
        // (`observed_at`, canonical runtime fields, `proxy.ready`,
        // `proxy.version`,
        // `proxy.base_url`, provider account counts, provider probe detail,
        // and the provider reason code) so a passing fixture cannot silently
        // diverge from the upstream serialization again.
        let body = r#"{
            "schema_version": 1,
            "observed_at": "2026-10-02T12:00:00Z",
            "runtime": {
                "generation": 17,
                "digest_prefix": "0123456789ab",
                "reload": "idle",
                "tasks": "4/4",
                "db": "ok",
                "retiring": 0
            },
            "proxy": {
                "status": "ready",
                "available": true,
                "ready": true,
                "version": "0.9.1",
                "base_url": "http://127.0.0.1:11300",
                "uptime_seconds": 3600.5,
                "model_count": 7,
                "routable_accounts": 5,
                "enabled_accounts": 6,
                "reason_code": null
            },
            "providers": [
                {
                    "provider_id": "openai",
                    "status": "degraded",
                    "last_observation": "stale",
                    "enabled_accounts": 2,
                    "total_accounts": 2,
                    "routable_accounts": 1,
                    "backoff_accounts": 1,
                    "unavailable_accounts": 1,
                    "model_count": 3,
                    "last_probe_age_seconds": 2,
                    "last_probe_latency_ms": 31,
                    "last_probe_status_code": 200,
                    "reason_code": "slow"
                }
            ]
        }"#
        .replace('\n', "");
        let (port, _requests, server) =
            server_routes(RouteReply::ok(summary_body("1h")), RouteReply::ok(body)).await;
        let (summary, health) = fetch_both(port, None).await;
        assert!(matches!(summary, EggpoolFetchOutcome::Online(_)));
        let EggpoolHealthFetchOutcome::Online(snapshot) = health else {
            panic!("the upstream-shaped provider-bearing payload must decode Online: {health:?}");
        };
        assert_eq!(snapshot.schema_version, 1);
        assert_eq!(snapshot.proxy, EggpoolProxyHealth::Ready);
        assert!(snapshot.available);
        // Account counts come from `proxy`, never from the provider rows or a
        // root-level field.
        assert_eq!(snapshot.routable_accounts, Some(5));
        assert_eq!(snapshot.enabled_accounts, Some(6));
        assert_eq!(snapshot.model_count, Some(7));
        assert_eq!(snapshot.uptime_seconds, Some(3600.5));
        // Provider identity and observation survive normalization from
        // `provider_id` / `last_observation`.
        assert_eq!(snapshot.providers.len(), 1);
        assert_eq!(snapshot.providers[0].id, "openai");
        assert_eq!(
            snapshot.providers[0].status,
            EggpoolProviderHealth::Degraded
        );
        assert_eq!(
            snapshot.providers[0].observation,
            Some(EggpoolProviderObservation::Stale)
        );
        // The provider's own reason code is not the proxy's, and is ignored.
        assert_eq!(snapshot.reason_code, None);
        server.abort();
    }

    #[tokio::test]
    async fn gregg_local_status_shape_is_not_a_supported_schema() {
        // The never-upstream Plan-152 shape used `id`/`observation` on provider
        // rows and root-level account counts. No alias preserves it: a
        // provider row without `provider_id` is a decode failure, and root
        // counts no longer populate the snapshot.
        let gregg_local = r#"{"schema_version":1,
            "proxy":{"status":"ready","available":true,"uptime_seconds":42.5,"model_count":3},
            "routable_accounts":5,"enabled_accounts":6,
            "providers":[{"id":"openai","status":"ready","observation":"verified"}]}"#
            .replace('\n', "");
        let (port, _requests, server) = server_routes(
            RouteReply::ok(summary_body("1h")),
            RouteReply::ok(gregg_local),
        )
        .await;
        let (summary, health) = fetch_both(port, None).await;
        assert!(matches!(summary, EggpoolFetchOutcome::Online(_)));
        assert_eq!(health, EggpoolHealthFetchOutcome::DecodeError);
        server.abort();

        // With no provider rows the proxy still decodes, but the misplaced
        // root counts must read as absent rather than being invented.
        let mislaid_counts = r#"{"schema_version":1,
            "proxy":{"status":"ready","available":true},
            "routable_accounts":5,"enabled_accounts":6,
            "providers":[]}"#
            .replace('\n', "");
        let (port, _requests, server) = server_routes(
            RouteReply::ok(summary_body("1h")),
            RouteReply::ok(mislaid_counts),
        )
        .await;
        let EggpoolHealthFetchOutcome::Online(snapshot) = fetch_both(port, None).await.1 else {
            panic!("a provider-free status payload still decodes");
        };
        assert_eq!(snapshot.proxy, EggpoolProxyHealth::Ready);
        assert_eq!(snapshot.routable_accounts, None);
        assert_eq!(snapshot.enabled_accounts, None);
        server.abort();
    }

    #[tokio::test]
    async fn health_never_renders_or_retains_the_configured_secret() {
        let (port, _requests, server) = server_routes(
            RouteReply::ok(summary_body("1h")),
            RouteReply::ok(canonical_status_body("ready", "ready", "verified")),
        )
        .await;
        let client = EggpoolClient::with_env_lookup(
            Duration::from_secs(2),
            Arc::new(|_| Some(OsString::from("secret-value"))),
        );
        let endpoint = endpoint(port, Some("KEY"));
        let health = client.fetch_health(&endpoint).await;
        assert!(matches!(health, EggpoolHealthFetchOutcome::Online(_)));
        assert!(!format!("{health:?}").contains("secret-value"));
        server.abort();
    }

    #[tokio::test]
    async fn unusable_health_credential_is_reported_without_sending() {
        let (port, mut requests, server) = server_routes(
            RouteReply::ok(summary_body("1h")),
            RouteReply::ok(canonical_status_body("ready", "ready", "verified")),
        )
        .await;
        let client = EggpoolClient::with_env_lookup(
            Duration::from_secs(2),
            Arc::new(|_| Some(OsString::from("bad\nvalue"))),
        );
        assert_eq!(
            client.fetch_health(&endpoint(port, Some("KEY"))).await,
            EggpoolHealthFetchOutcome::InvalidApiKey
        );
        assert!(requests.try_recv().is_err());
        server.abort();
    }

    #[tokio::test]
    async fn invalid_endpoint_is_reported_by_both_planes() {
        let client = EggpoolClient::new(Duration::from_secs(1));
        let endpoint = EggpoolEntry {
            host: "fe80::1%eth0".into(),
            port: 11300,
            ..endpoint(11300, None)
        };
        assert_eq!(
            client.fetch(&endpoint, EggpoolPeriod::Hour).await,
            EggpoolFetchOutcome::InvalidEndpoint
        );
        assert_eq!(
            client.fetch_health(&endpoint).await,
            EggpoolHealthFetchOutcome::InvalidEndpoint
        );
    }

    #[test]
    fn status_url_normalizes_bracketed_ipv6() {
        let endpoint = EggpoolEntry {
            host: "[2001:db8::1]".into(),
            port: 8080,
            ..endpoint(8080, None)
        };
        assert_eq!(
            status_url(&endpoint).unwrap().as_str(),
            "http://[2001:db8::1]:8080/api/status"
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
