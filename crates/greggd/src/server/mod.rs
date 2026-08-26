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

/// Current time as milliseconds since the Unix epoch.
///
/// A clock behind the epoch would collapse every timestamp to `0` and defeat
/// age-based staleness detection, so that condition is logged loudly.
#[allow(clippy::cast_possible_truncation)]
fn now_unix_ms() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_millis() as u64,
        Err(error) => {
            tracing::warn!(
                %error,
                "system clock precedes the Unix epoch; staleness checks will be inaccurate until corrected"
            );
            0
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
}

#[derive(Debug)]
struct PublishedState {
    snapshot: Option<Arc<StatusSnapshot>>,
    snapshot_v2: Option<Arc<StatusPayloadV2>>,
    last_observed_at_unix_ms: Option<u64>,
    health: HealthResponse,
    health_v2: HealthResponseV2,
    consecutive_failures: u32,
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
                last_observed_at_unix_ms: None,
                health: HealthResponse::warming(),
                health_v2: HealthResponseV2::warming(),
                consecutive_failures: 0,
            })),
            max_consecutive_failures,
            max_snapshot_age,
        }
    }

    /// Publish new v1 and v2 snapshots and mark the server ready.
    pub async fn update_snapshot(&self, snap: StatusSnapshot, payload_v2: StatusPayloadV2) {
        let observed_at_unix_ms = snap
            .observed_at_unix_ms
            .max(payload_v2.snapshot.observed_at_unix_ms);
        let health = HealthResponse::ready(snap.clone());
        let health_v2 = HealthResponseV2::ready(payload_v2.snapshot.clone());
        let arc_snap = Arc::new(snap);
        let arc_snap_v2 = Arc::new(payload_v2);
        let mut state = self.published.write().await;
        state.snapshot = Some(arc_snap);
        state.snapshot_v2 = Some(arc_snap_v2);
        state.health = health;
        state.health_v2 = health_v2;
        state.last_observed_at_unix_ms = Some(observed_at_unix_ms);
        state.consecutive_failures = 0;
    }

    /// Publish a v2 snapshot only, without a v1 snapshot.
    ///
    /// Used on Windows where v1 is not supported.
    pub async fn update_snapshot_v2_only(&self, payload_v2: StatusPayloadV2) {
        let observed_at_unix_ms = payload_v2.snapshot.observed_at_unix_ms;
        let health_v2 = HealthResponseV2::ready(payload_v2.snapshot.clone());
        let mut state = self.published.write().await;
        state.snapshot = None;
        state.snapshot_v2 = Some(Arc::new(payload_v2));
        state.health = HealthResponse::failed(
            gregg_protocol::HealthCategory::NotServing,
            V1_UNAVAILABLE_MESSAGE,
        );
        state.health_v2 = health_v2;
        state.last_observed_at_unix_ms = Some(observed_at_unix_ms);
        state.consecutive_failures = 0;
    }

    /// Publish a v1 snapshot only, marking v2 as unavailable.
    ///
    /// This is retained for the sampler's v1-only compatibility path. Normal
    /// platforms publish both versions, while Windows publishes v2 only.
    pub async fn update_snapshot_v1_only(&self, snap: StatusSnapshot) {
        let observed_at_unix_ms = snap.observed_at_unix_ms;
        let health = HealthResponse::ready(snap.clone());
        let mut state = self.published.write().await;
        state.snapshot = Some(Arc::new(snap));
        state.snapshot_v2 = None;
        state.health = health;
        state.health_v2 = HealthResponseV2::failed(
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
        state.last_observed_at_unix_ms = None;
        state.health = HealthResponse::warming();
        state.health_v2 = HealthResponseV2::warming();
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
                HealthResponse::failed(gregg_protocol::HealthCategory::CollectorFailure, msg);
        }
        if state.health_v2.category != Some(gregg_protocol::HealthCategory::NotServing) {
            state.health_v2 =
                HealthResponseV2::failed(gregg_protocol::HealthCategory::CollectorFailure, msg);
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

    async fn v1_status_data(
        &self,
        now_unix_ms: u64,
    ) -> (Option<Arc<StatusSnapshot>>, HealthResponse, bool) {
        let state = self.published.read().await;
        let snapshot_is_stale = self.is_stale(&state, now_unix_ms);
        let mut health = state.health.clone();
        // A stored `ready` health response must never accompany a 503 for a
        // stale snapshot; report the staleness as a collector failure instead.
        if snapshot_is_stale && health.state == ReadinessState::Ready {
            health = HealthResponse::failed(
                gregg_protocol::HealthCategory::CollectorFailure,
                "cached snapshot is stale",
            );
        }
        (state.snapshot.clone(), health, snapshot_is_stale)
    }

    async fn v2_status_data(
        &self,
        now_unix_ms: u64,
    ) -> (Option<Arc<StatusPayloadV2>>, HealthResponseV2, bool) {
        let state = self.published.read().await;
        let snapshot_is_stale = self.is_stale(&state, now_unix_ms);
        let mut health_v2 = state.health_v2.clone();
        if snapshot_is_stale && health_v2.state == ReadinessState::Ready {
            health_v2 = HealthResponseV2::failed(
                gregg_protocol::HealthCategory::CollectorFailure,
                "cached snapshot is stale",
            );
        }
        (state.snapshot_v2.clone(), health_v2, snapshot_is_stale)
    }

    fn is_stale(&self, state: &PublishedState, now_unix_ms: u64) -> bool {
        if self.max_consecutive_failures > 0 {
            let failures = state.consecutive_failures;
            if failures >= self.max_consecutive_failures {
                return true;
            }
        }
        if !self.max_snapshot_age.is_zero() {
            if let Some(observed_at_unix_ms) = state.last_observed_at_unix_ms {
                let age_ms = now_unix_ms.checked_sub(observed_at_unix_ms);
                if age_ms.map_or(true, |age| {
                    u128::from(age) >= self.max_snapshot_age.as_millis()
                }) {
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
        self.published.read().await.health.clone()
    }

    /// Clone of the current v2 health response.
    pub async fn health_v2(&self) -> HealthResponseV2 {
        self.published.read().await.health_v2.clone()
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
    let addr = listener.local_addr().expect("listener has local addr");

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
    let (snap, health_state, snapshot_is_stale) = state.v1_status_data(now).await;
    if let Some(snap) = snap {
        if snapshot_is_stale {
            return health_response(&health_state, StatusCode::SERVICE_UNAVAILABLE);
        }
        let body = match serde_json::to_vec(&*snap) {
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
        return (StatusCode::OK, [("content-type", "application/json")], body).into_response();
    }
    health_response(&health_state, StatusCode::SERVICE_UNAVAILABLE)
}

/// GET `/healthz` — returns readiness/health as compact JSON.
///
/// Returns `200` when ready and the snapshot is fresh. Returns `503` when
/// warming, failed, or when the snapshot is stale.
async fn health_handler(State(state): State<ServerState>) -> Response {
    let now = now_unix_ms();

    let (_, health_state, snapshot_is_stale) = state.v1_status_data(now).await;
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
    let (snap, health_state, snapshot_is_stale) = state.v2_status_data(now).await;
    if let Some(snap) = snap {
        if snapshot_is_stale {
            return health_response_v2(&health_state, StatusCode::SERVICE_UNAVAILABLE);
        }
        let body = match serde_json::to_vec(&*snap) {
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
        return (StatusCode::OK, [("content-type", "application/json")], body).into_response();
    }
    health_response_v2(&health_state, StatusCode::SERVICE_UNAVAILABLE)
}

/// GET `/v2/healthz` — returns v2 readiness/health as compact JSON.
async fn health_handler_v2(State(state): State<ServerState>) -> Response {
    let now = now_unix_ms();

    let (_, health_state, snapshot_is_stale) = state.v2_status_data(now).await;
    let status =
        if health_state.state == gregg_protocol::ReadinessState::Ready && !snapshot_is_stale {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        };
    health_response_v2(&health_state, status)
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
