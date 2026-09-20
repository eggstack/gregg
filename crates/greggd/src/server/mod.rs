//! HTTP server for the `greggd` daemon.
//!
//! Exposes read-only status and health endpoints:
//!
//! - `GET /` and `GET /v1/status` — latest v1 status snapshot as compact JSON.
//! - `GET /v2/status` — latest flat v2 status payload, including optional drives.
//! - `GET /healthz` — readiness and health information.
//!
//! All other methods or paths return `404`. No TLS, cookies, sessions,
//! multipart handling, WebSocket upgrade, compression, or static-file serving
//! is supported.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use gregg_protocol::v2::{HealthResponseV2, StatusPayloadV2};
use gregg_protocol::{HealthResponse, ReadinessState, StatusSnapshot};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, RwLock};
use tracing::info;

use crate::server::error::{ServerConfigError, ServerError};

pub mod error;

const V1_UNAVAILABLE_MESSAGE: &str = "schema v1 status is unavailable on this platform";

fn serialize_status_v1(snapshot: &StatusSnapshot) -> Result<Bytes, serde_json::Error> {
    serde_json::to_vec(snapshot).map(Bytes::from)
}

fn serialize_status_v2(snapshot: &StatusPayloadV2) -> Result<Bytes, serde_json::Error> {
    serde_json::to_vec(snapshot).map(Bytes::from)
}

/// Current time as milliseconds since the Unix epoch.
///
/// A clock behind the epoch cannot provide a meaningful non-negative age, so
/// callers treat age-based staleness as true until the clock is corrected.
#[allow(clippy::cast_possible_truncation)]
fn now_unix_ms() -> Option<u64> {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => Some(duration.as_millis() as u64),
        Err(error) => {
            tracing::debug!(
                %error,
                "system clock precedes the Unix epoch; cached snapshots remain age-uncheckable until corrected"
            );
            None
        }
    }
}

/// HTTP server bind configuration.
///
/// Defaults are safe for local development; production deployments should
/// explicitly set `host` and `port`.
#[derive(Debug, Clone)]
pub struct Config {
    /// Address to bind to.
    pub host: IpAddr,
    /// TCP port to listen on.
    pub port: u16,
    /// Sampling cadence in milliseconds exposed in health responses.
    pub sample_interval_ms: u64,
    /// Maximum number of consecutive collector failures before the daemon
    /// considers its snapshot stale and stops serving it from `/v1/status`.
    /// A value of `0` means the snapshot is never considered stale due to
    /// failure count alone.
    pub max_consecutive_failures: u32,
    /// Maximum age of a snapshot before it is considered stale and not
    /// served from `/v1/status`. A value of `Duration::ZERO` means the
    /// snapshot is never considered stale due to age alone.
    pub max_snapshot_age: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
            port: 11310,
            sample_interval_ms: 1000,
            max_consecutive_failures: 0,
            max_snapshot_age: Duration::ZERO,
        }
    }
}

impl Config {
    /// Validate configuration fields.
    ///
    /// # Errors
    ///
    /// Returns [`ServerConfigError::InvalidPort`] if `port` is outside
    /// `1..=65535` or [`ServerConfigError::InvalidSampleInterval`] if
    /// `sample_interval_ms` is outside `250..=60000`.
    pub fn validate(&self) -> Result<(), ServerConfigError> {
        if self.port == 0 {
            return Err(ServerConfigError::InvalidPort(self.port));
        }
        if self.sample_interval_ms < 250 || self.sample_interval_ms > 60000 {
            return Err(ServerConfigError::InvalidSampleInterval(
                self.sample_interval_ms,
            ));
        }
        Ok(())
    }

    /// Returns the resolved socket address the server will bind to.
    #[must_use]
    pub fn socket_addr(&self) -> SocketAddr {
        SocketAddr::new(self.host, self.port)
    }
}

/// Shared server state.
#[derive(Debug, Clone)]
pub struct ServerState {
    published: Arc<RwLock<PublishedState>>,
    /// Maximum consecutive failures before snapshot is considered stale.
    max_consecutive_failures: u32,
    /// Maximum snapshot age before it is considered stale.
    max_snapshot_age: Duration,
    #[cfg(test)]
    v1_status_serializations: Arc<std::sync::atomic::AtomicUsize>,
    #[cfg(test)]
    v2_status_serializations: Arc<std::sync::atomic::AtomicUsize>,
}

#[derive(Debug)]
struct PublishedState {
    snapshot: Option<Arc<StatusSnapshot>>,
    snapshot_v2: Option<Arc<StatusPayloadV2>>,
    status_bytes: Option<Bytes>,
    status_bytes_v2: Option<Bytes>,
    last_observed_at_unix_ms: Option<u64>,
    health: HealthMetadata,
    health_v2: HealthMetadata,
    consecutive_failures: u32,
}

#[derive(Debug, Clone)]
struct HealthMetadata {
    state: ReadinessState,
    category: Option<gregg_protocol::HealthCategory>,
    message: Option<String>,
}

impl HealthMetadata {
    fn warming() -> Self {
        Self {
            state: ReadinessState::Warming,
            category: Some(gregg_protocol::HealthCategory::Warming),
            message: Some("collector warming up".into()),
        }
    }

    fn ready() -> Self {
        Self {
            state: ReadinessState::Ready,
            category: None,
            message: None,
        }
    }

    fn failed(category: gregg_protocol::HealthCategory, message: &str) -> Self {
        Self {
            state: ReadinessState::Failed,
            category: Some(category),
            message: Some(message.into()),
        }
    }

    fn v1_response(&self, snapshot: Option<&StatusSnapshot>) -> HealthResponse {
        match self.state {
            ReadinessState::Ready => snapshot.map_or_else(
                || {
                    HealthResponse::failed(
                        gregg_protocol::HealthCategory::CollectorFailure,
                        "snapshot unavailable",
                    )
                },
                |snapshot| HealthResponse::ready(snapshot.clone()),
            ),
            ReadinessState::Warming => HealthResponse::warming_with_message(
                self.message.as_deref().unwrap_or("collector warming up"),
            ),
            ReadinessState::Failed => HealthResponse::failed(
                self.category
                    .unwrap_or(gregg_protocol::HealthCategory::CollectorFailure),
                self.message.as_deref().unwrap_or("collector failure"),
            ),
        }
    }

    fn v2_response(
        &self,
        snapshot: Option<&gregg_protocol::v2::StatusSnapshotV2>,
    ) -> HealthResponseV2 {
        match self.state {
            ReadinessState::Ready => snapshot.map_or_else(
                || {
                    HealthResponseV2::failed(
                        gregg_protocol::HealthCategory::CollectorFailure,
                        "snapshot unavailable",
                    )
                },
                |snapshot| HealthResponseV2::ready(snapshot.clone()),
            ),
            ReadinessState::Warming => HealthResponseV2::warming_with_message(
                self.message.as_deref().unwrap_or("collector warming up"),
            ),
            ReadinessState::Failed => HealthResponseV2::failed(
                self.category
                    .unwrap_or(gregg_protocol::HealthCategory::CollectorFailure),
                self.message.as_deref().unwrap_or("collector failure"),
            ),
        }
    }
}

enum StatusDataV1 {
    FreshCached(Bytes),
    FreshTyped(Arc<StatusSnapshot>),
    Unavailable(Box<HealthResponse>),
}

enum StatusDataV2 {
    FreshCached(Bytes),
    FreshTyped(Arc<StatusPayloadV2>),
    Unavailable(Box<HealthResponseV2>),
}

impl Default for ServerState {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerState {
    /// Create a new instance in the warming state.
    #[must_use]
    pub fn new() -> Self {
        Self::with_stale_policy(0, Duration::ZERO)
    }

    /// Create a new instance with the given stale-snapshot policy.
    #[must_use]
    pub fn with_stale_policy(max_consecutive_failures: u32, max_snapshot_age: Duration) -> Self {
        Self {
            published: Arc::new(RwLock::new(PublishedState {
                snapshot: None,
                snapshot_v2: None,
                status_bytes: None,
                status_bytes_v2: None,
                last_observed_at_unix_ms: None,
                health: HealthMetadata::warming(),
                health_v2: HealthMetadata::warming(),
                consecutive_failures: 0,
            })),
            max_consecutive_failures,
            max_snapshot_age,
            #[cfg(test)]
            v1_status_serializations: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            #[cfg(test)]
            v2_status_serializations: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    #[allow(clippy::unused_self)]
    fn serialize_v1_status(&self, snapshot: &StatusSnapshot) -> Result<Bytes, serde_json::Error> {
        #[cfg(test)]
        self.v1_status_serializations
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        serialize_status_v1(snapshot)
    }

    #[allow(clippy::unused_self)]
    fn serialize_v2_status(&self, snapshot: &StatusPayloadV2) -> Result<Bytes, serde_json::Error> {
        #[cfg(test)]
        self.v2_status_serializations
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        serialize_status_v2(snapshot)
    }

    /// Publish new v1 and v2 snapshots and mark the server ready.
    pub async fn update_snapshot(&self, snap: StatusSnapshot, payload_v2: StatusPayloadV2) {
        self.update_snapshot_arcs(Arc::new(snap), Arc::new(payload_v2))
            .await;
    }

    /// Publish already-owned snapshots without cloning them across the
    /// sampler/server boundary.
    pub(crate) async fn update_snapshot_arcs(
        &self,
        snap: Arc<StatusSnapshot>,
        payload_v2: Arc<StatusPayloadV2>,
    ) {
        let observed_at_unix_ms = snap
            .observed_at_unix_ms
            .max(payload_v2.snapshot.observed_at_unix_ms);
        let status_bytes = self.serialize_v1_status(&snap).ok();
        let status_bytes_v2 = self.serialize_v2_status(&payload_v2).ok();
        let mut state = self.published.write().await;
        state.snapshot = Some(snap);
        state.snapshot_v2 = Some(payload_v2);
        state.status_bytes = status_bytes;
        state.status_bytes_v2 = status_bytes_v2;
        state.health = HealthMetadata::ready();
        state.health_v2 = HealthMetadata::ready();
        state.last_observed_at_unix_ms = Some(observed_at_unix_ms);
        state.consecutive_failures = 0;
    }

    /// Publish a v2 snapshot only, without a v1 snapshot.
    ///
    /// Used on Windows where v1 is not supported.
    pub async fn update_snapshot_v2_only(&self, payload_v2: StatusPayloadV2) {
        self.update_snapshot_v2_only_arc(Arc::new(payload_v2)).await;
    }

    /// Publish an already-owned v2 snapshot without cloning it.
    pub(crate) async fn update_snapshot_v2_only_arc(&self, payload_v2: Arc<StatusPayloadV2>) {
        let observed_at_unix_ms = payload_v2.snapshot.observed_at_unix_ms;
        let status_bytes_v2 = self.serialize_v2_status(&payload_v2).ok();
        let mut state = self.published.write().await;
        state.snapshot = None;
        state.snapshot_v2 = Some(payload_v2);
        state.status_bytes = None;
        state.status_bytes_v2 = status_bytes_v2;
        state.health = HealthMetadata::failed(
            gregg_protocol::HealthCategory::NotServing,
            V1_UNAVAILABLE_MESSAGE,
        );
        state.health_v2 = HealthMetadata::ready();
        state.last_observed_at_unix_ms = Some(observed_at_unix_ms);
        state.consecutive_failures = 0;
    }

    /// Publish a v1 snapshot only, marking v2 as unavailable.
    ///
    /// This is retained for the sampler's v1-only compatibility path. Normal
    /// platforms publish both versions, while Windows publishes v2 only.
    pub async fn update_snapshot_v1_only(&self, snap: StatusSnapshot) {
        self.update_snapshot_v1_only_arc(Arc::new(snap)).await;
    }

    /// Publish an already-owned v1 snapshot without cloning it.
    pub(crate) async fn update_snapshot_v1_only_arc(&self, snap: Arc<StatusSnapshot>) {
        let observed_at_unix_ms = snap.observed_at_unix_ms;
        let status_bytes = self.serialize_v1_status(&snap).ok();
        let mut state = self.published.write().await;
        state.snapshot = Some(snap);
        state.snapshot_v2 = None;
        state.status_bytes = status_bytes;
        state.status_bytes_v2 = None;
        state.health = HealthMetadata::ready();
        state.health_v2 = HealthMetadata::failed(
            gregg_protocol::HealthCategory::NotServing,
            "schema v2 status is unavailable from this sampler",
        );
        state.last_observed_at_unix_ms = Some(observed_at_unix_ms);
        state.consecutive_failures = 0;
    }

    /// Set the daemon to warming state.
    pub async fn set_warming(&self) {
        let mut state = self.published.write().await;
        state.snapshot = None;
        state.snapshot_v2 = None;
        state.status_bytes = None;
        state.status_bytes_v2 = None;
        state.last_observed_at_unix_ms = None;
        state.health = HealthMetadata::warming();
        state.health_v2 = HealthMetadata::warming();
        state.consecutive_failures = 0;
    }

    /// Set the daemon to failed state with a diagnostic message.
    ///
    /// The existing snapshot is preserved so `/v1/status` and `/v2/status`
    /// can continue serving it as stale data if the staleness policy permits.
    pub async fn set_failed(&self, msg: &str) {
        let mut state = self.published.write().await;
        state.consecutive_failures = state.consecutive_failures.saturating_add(1);
        let prev = state.consecutive_failures;
        if state.health.category != Some(gregg_protocol::HealthCategory::NotServing) {
            state.health =
                HealthMetadata::failed(gregg_protocol::HealthCategory::CollectorFailure, msg);
        }
        if state.health_v2.category != Some(gregg_protocol::HealthCategory::NotServing) {
            state.health_v2 =
                HealthMetadata::failed(gregg_protocol::HealthCategory::CollectorFailure, msg);
        }
        // Snapshot is deliberately NOT cleared here. The stale-snapshot
        // policy in the status handler decides whether to serve it.
        tracing::debug!(
            consecutive_failures = prev,
            max = self.max_consecutive_failures,
            "server failure recorded"
        );
    }

    /// Return the current consecutive failure count.
    #[must_use]
    pub async fn consecutive_failures(&self) -> u32 {
        self.published.read().await.consecutive_failures
    }

    async fn v1_status_data(&self, now_unix_ms: Option<u64>) -> StatusDataV1 {
        let state = self.published.read().await;
        let snapshot_is_stale = self.is_stale(&state, now_unix_ms);
        if let Some(snapshot) = &state.snapshot {
            if snapshot_is_stale {
                return StatusDataV1::Unavailable(Box::new(HealthResponse::failed(
                    gregg_protocol::HealthCategory::CollectorFailure,
                    "cached snapshot is stale",
                )));
            }
            return state.status_bytes.as_ref().map_or_else(
                || StatusDataV1::FreshTyped(Arc::clone(snapshot)),
                |body| StatusDataV1::FreshCached(body.clone()),
            );
        }
        StatusDataV1::Unavailable(Box::new(state.health.v1_response(None)))
    }

    async fn v2_status_data(&self, now_unix_ms: Option<u64>) -> StatusDataV2 {
        let state = self.published.read().await;
        let snapshot_is_stale = self.is_stale(&state, now_unix_ms);
        if let Some(snapshot) = &state.snapshot_v2 {
            if snapshot_is_stale {
                return StatusDataV2::Unavailable(Box::new(HealthResponseV2::failed(
                    gregg_protocol::HealthCategory::CollectorFailure,
                    "cached snapshot is stale",
                )));
            }
            return state.status_bytes_v2.as_ref().map_or_else(
                || StatusDataV2::FreshTyped(Arc::clone(snapshot)),
                |body| StatusDataV2::FreshCached(body.clone()),
            );
        }
        StatusDataV2::Unavailable(Box::new(state.health_v2.v2_response(None)))
    }

    async fn v1_health_data(&self, now_unix_ms: Option<u64>) -> (HealthResponse, bool) {
        let state = self.published.read().await;
        let snapshot_is_stale = self.is_stale(&state, now_unix_ms);
        let health = if snapshot_is_stale && state.health.state == ReadinessState::Ready {
            HealthResponse::failed(
                gregg_protocol::HealthCategory::CollectorFailure,
                "cached snapshot is stale",
            )
        } else {
            state.health.v1_response(state.snapshot.as_deref())
        };
        (health, snapshot_is_stale)
    }

    async fn v2_health_data(&self, now_unix_ms: Option<u64>) -> (HealthResponseV2, bool) {
        let state = self.published.read().await;
        let snapshot_is_stale = self.is_stale(&state, now_unix_ms);
        let health = if snapshot_is_stale && state.health_v2.state == ReadinessState::Ready {
            HealthResponseV2::failed(
                gregg_protocol::HealthCategory::CollectorFailure,
                "cached snapshot is stale",
            )
        } else {
            state.health_v2.v2_response(
                state
                    .snapshot_v2
                    .as_deref()
                    .map(|payload| &payload.snapshot),
            )
        };
        (health, snapshot_is_stale)
    }

    fn is_stale(&self, state: &PublishedState, now_unix_ms: Option<u64>) -> bool {
        if self.max_consecutive_failures > 0 {
            let failures = state.consecutive_failures;
            if failures >= self.max_consecutive_failures {
                return true;
            }
        }
        if !self.max_snapshot_age.is_zero() {
            let Some(now_unix_ms) = now_unix_ms else {
                return true;
            };
            if let Some(observed_at_unix_ms) = state.last_observed_at_unix_ms {
                // A `None` age means `observed_at` lies in the future
                // (backward clock jump after sampling); treat a
                // from-the-future snapshot as stale rather than fresh.
                let age_ms = now_unix_ms.checked_sub(observed_at_unix_ms);
                if age_ms.is_none_or(|age| u128::from(age) >= self.max_snapshot_age.as_millis()) {
                    return true;
                }
            }
        }
        false
    }

    /// Clone of the latest snapshot, if available.
    pub async fn snapshot(&self) -> Option<Arc<StatusSnapshot>> {
        self.published.read().await.snapshot.clone()
    }

    /// Clone of the latest v2 snapshot, if available.
    pub async fn snapshot_v2(&self) -> Option<Arc<StatusPayloadV2>> {
        self.published.read().await.snapshot_v2.clone()
    }

    /// Clone of the current health response.
    pub async fn health(&self) -> HealthResponse {
        let state = self.published.read().await;
        state.health.v1_response(state.snapshot.as_deref())
    }

    /// Clone of the current v2 health response.
    pub async fn health_v2(&self) -> HealthResponseV2 {
        let state = self.published.read().await;
        state.health_v2.v2_response(
            state
                .snapshot_v2
                .as_deref()
                .map(|payload| &payload.snapshot),
        )
    }
}

/// Run the HTTP server until `shutdown` fires.
///
/// The caller must provide an already-bound [`TcpListener`] so that bind
/// failures are surfaced before any tasks are spawned.
///
/// # Errors
///
/// Returns [`ServerError::Runtime`] if the server encounters an I/O error
/// while running.
pub async fn serve(
    listener: TcpListener,
    state: ServerState,
    mut shutdown: broadcast::Receiver<()>,
) -> Result<(), ServerError> {
    let addr = listener.local_addr().map_err(ServerError::Runtime)?;

    let app = Router::new()
        .route("/", get(status_handler))
        .route("/v1/status", get(status_handler))
        .route("/v2/status", get(status_handler_v2))
        .route("/healthz", get(health_handler))
        .route("/v2/healthz", get(health_handler_v2))
        .fallback(fallback_handler)
        .with_state(state);

    info!("greggd listening on {addr}");

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = shutdown.recv().await;
            info!("shutdown signal received, stopping HTTP server");
        })
        .await
        .map_err(ServerError::Runtime)
}

/// GET `/` and `/v1/status` — returns the latest snapshot as compact JSON.
///
/// When the server is still warming up, returns `503` with the health
/// response so clients can surface readiness diagnostics.
///
/// When a collector failure has occurred but the last valid snapshot is not
/// yet stale according to the policy, the snapshot is served with its
/// original `observed_at_unix_ms` timestamp (200 OK). Once the snapshot is
/// stale, `503` is returned.
async fn status_handler(State(state): State<ServerState>) -> Response {
    let now = now_unix_ms();
    match state.v1_status_data(now).await {
        StatusDataV1::FreshCached(body) => cached_status_response(body),
        StatusDataV1::FreshTyped(snapshot) => match state.serialize_v1_status(&snapshot) {
            Ok(body) => cached_status_response(body),
            Err(error) => serialization_error_response(&error),
        },
        StatusDataV1::Unavailable(health) => {
            health_response(&health, StatusCode::SERVICE_UNAVAILABLE)
        }
    }
}

/// GET `/healthz` — returns readiness/health as compact JSON.
///
/// Returns `200` when ready and the snapshot is fresh. Returns `503` when
/// warming, failed, or when the snapshot is stale.
async fn health_handler(State(state): State<ServerState>) -> Response {
    let now = now_unix_ms();

    let (health_state, snapshot_is_stale) = state.v1_health_data(now).await;
    let status =
        if health_state.state == gregg_protocol::ReadinessState::Ready && !snapshot_is_stale {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        };
    health_response(&health_state, status)
}

/// Any non-matched route returns `404`.
async fn fallback_handler(method: Method, uri: axum::http::Uri) -> (StatusCode, String) {
    (StatusCode::NOT_FOUND, format!("{method} {uri} not found"))
}

fn health_response(health: &HealthResponse, status: StatusCode) -> Response {
    let body = match serde_json::to_vec(&health) {
        Ok(body) => body,
        Err(e) => {
            let error_body = serde_json::to_vec(&serde_json::json!({"error": e.to_string()}))
                .unwrap_or_else(|_| b"{\"error\":\"serialization failed\"}".to_vec());
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                [("content-type", "application/json")],
                error_body,
            )
                .into_response();
        }
    };
    (status, [("content-type", "application/json")], body).into_response()
}

/// GET `/v2/status` — returns the latest v2 snapshot as compact JSON.
///
/// When the server is still warming up, returns `503` with the v2 health
/// response. When a collector failure has occurred but the last valid
/// snapshot is not yet stale, the snapshot is served (200 OK). Once stale,
/// `503` is returned.
async fn status_handler_v2(State(state): State<ServerState>) -> Response {
    let now = now_unix_ms();
    match state.v2_status_data(now).await {
        StatusDataV2::FreshCached(body) => cached_status_response(body),
        StatusDataV2::FreshTyped(snapshot) => match state.serialize_v2_status(&snapshot) {
            Ok(body) => cached_status_response(body),
            Err(error) => serialization_error_response(&error),
        },
        StatusDataV2::Unavailable(health) => {
            health_response_v2(&health, StatusCode::SERVICE_UNAVAILABLE)
        }
    }
}

/// GET `/v2/healthz` — returns v2 readiness/health as compact JSON.
async fn health_handler_v2(State(state): State<ServerState>) -> Response {
    let now = now_unix_ms();

    let (health_state, snapshot_is_stale) = state.v2_health_data(now).await;
    let status =
        if health_state.state == gregg_protocol::ReadinessState::Ready && !snapshot_is_stale {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        };
    health_response_v2(&health_state, status)
}

fn cached_status_response(body: Bytes) -> Response {
    (StatusCode::OK, [("content-type", "application/json")], body).into_response()
}

fn serialization_error_response(error: &serde_json::Error) -> Response {
    let error_body = serde_json::to_vec(&serde_json::json!({"error": error.to_string()}))
        .unwrap_or_else(|_| b"{\"error\":\"serialization failed\"}".to_vec());
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        [("content-type", "application/json")],
        error_body,
    )
        .into_response()
}

fn health_response_v2(health: &HealthResponseV2, status: StatusCode) -> Response {
    let body = match serde_json::to_vec(&health) {
        Ok(body) => body,
        Err(e) => {
            let error_body = serde_json::to_vec(&serde_json::json!({"error": e.to_string()}))
                .unwrap_or_else(|_| b"{\"error\":\"serialization failed\"}".to_vec());
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                [("content-type", "application/json")],
                error_body,
            )
                .into_response();
        }
    };
    (status, [("content-type", "application/json")], body).into_response()
}

#[cfg(test)]
mod tests;
