//! HTTP server for the `greggd` daemon.
//!
//! Exposes three read-only endpoints:
//!
//! - `GET /` and `GET /v1/status` — latest status snapshot as compact JSON.
//! - `GET /healthz` — readiness and health information.
//!
//! All other methods or paths return `404`. No TLS, cookies, sessions,
//! multipart handling, WebSocket upgrade, compression, or static-file serving
//! is supported.

use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use gregg_protocol::{HealthResponse, StatusSnapshot};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, RwLock};
use tracing::info;

use crate::server::error::{ServerConfigError, ServerError};

pub mod error;

/// Current time as milliseconds since the Unix epoch.
#[allow(
    clippy::cast_possible_truncation,
    reason = "millis from SystemTime is well within u64 range"
)]
fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
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

    #[must_use]
    pub fn socket_addr(&self) -> SocketAddr {
        SocketAddr::new(self.host, self.port)
    }
}

/// Shared server state.
#[derive(Debug, Clone)]
pub struct ServerState {
    /// Latest status snapshot. Preserved across failure transitions so
    /// `/v1/status` can continue serving stale data.
    snapshot: Arc<RwLock<Option<Arc<StatusSnapshot>>>>,
    /// Readiness flag: `true` once a valid snapshot is available and not
    /// stale.
    ready: Arc<AtomicBool>,
    /// Current health response.
    health: Arc<RwLock<HealthResponse>>,
    /// Consecutive collector failure count. Reset to `0` on success.
    consecutive_failures: Arc<AtomicU32>,
    /// Maximum consecutive failures before snapshot is considered stale.
    max_consecutive_failures: u32,
    /// Maximum snapshot age before it is considered stale.
    max_snapshot_age: Duration,
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
            snapshot: Arc::new(RwLock::new(None)),
            ready: Arc::new(AtomicBool::new(false)),
            health: Arc::new(RwLock::new(HealthResponse::warming())),
            consecutive_failures: Arc::new(AtomicU32::new(0)),
            max_consecutive_failures,
            max_snapshot_age,
        }
    }

    /// Publish a new snapshot and mark the server ready.
    pub async fn update_snapshot(&self, snap: StatusSnapshot) {
        let health = HealthResponse::ready(snap.clone());
        let arc_snap = Arc::new(snap);
        {
            let mut guard = self.snapshot.write().await;
            *guard = Some(arc_snap);
        }
        {
            let mut guard = self.health.write().await;
            *guard = health;
        }
        self.consecutive_failures.store(0, Ordering::Release);
        self.ready.store(true, Ordering::Release);
    }

    /// Set the daemon to warming state.
    pub async fn set_warming(&self) {
        self.ready.store(false, Ordering::Release);
        self.consecutive_failures.store(0, Ordering::Release);
        {
            let mut guard = self.snapshot.write().await;
            *guard = None;
        }
        {
            let mut guard = self.health.write().await;
            *guard = HealthResponse::warming();
        }
    }

    /// Set the daemon to failed state with a diagnostic message.
    ///
    /// The existing snapshot is preserved so `/v1/status` can continue
    /// serving it as stale data if the staleness policy permits.
    pub async fn set_failed(&self, msg: &str) {
        let prev = self.consecutive_failures.fetch_add(1, Ordering::AcqRel) + 1;
        self.ready.store(false, Ordering::Release);
        {
            let mut guard = self.health.write().await;
            *guard = HealthResponse::failed(gregg_protocol::HealthCategory::CollectorFailure, msg);
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
    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures.load(Ordering::Acquire)
    }

    /// Check whether the current snapshot is stale according to the policy.
    ///
    /// A snapshot is stale if:
    /// - `max_consecutive_failures > 0` and the failure count meets or
    ///   exceeds it, OR
    /// - `max_snapshot_age > Duration::ZERO` and the snapshot's
    ///   `observed_at_unix_ms` is older than `now - max_snapshot_age`.
    async fn is_snapshot_stale(&self, now_unix_ms: u64) -> bool {
        if self.max_consecutive_failures > 0 {
            let failures = self.consecutive_failures.load(Ordering::Acquire);
            if failures >= self.max_consecutive_failures {
                return true;
            }
        }
        if !self.max_snapshot_age.is_zero() {
            let guard = self.snapshot.read().await;
            if let Some(ref snap) = *guard {
                let age_ms = now_unix_ms.saturating_sub(snap.observed_at_unix_ms);
                #[allow(
                    clippy::cast_possible_truncation,
                    reason = "millis from Duration is well within u64 range"
                )]
                if age_ms >= self.max_snapshot_age.as_millis() as u64 {
                    return true;
                }
            }
        }
        false
    }

    /// Clone of the latest snapshot, if available.
    pub async fn snapshot(&self) -> Option<Arc<StatusSnapshot>> {
        self.snapshot.read().await.clone()
    }

    /// Clone of the current health response.
    pub async fn health(&self) -> HealthResponse {
        self.health.read().await.clone()
    }
}

/// Run the HTTP server until `shutdown` fires.
///
/// # Errors
///
/// Returns [`ServerError::Bind`] if the address cannot be bound, or
/// [`ServerError::Runtime`] if the server encounters an I/O error.
pub async fn serve(
    config: Config,
    state: ServerState,
    mut shutdown: broadcast::Receiver<()>,
) -> Result<(), ServerError> {
    let addr = config.socket_addr();

    let app = Router::new()
        .route("/", get(status_handler))
        .route("/v1/status", get(status_handler))
        .route("/healthz", get(health_handler))
        .fallback(fallback_handler)
        .with_state(state);

    let listener = TcpListener::bind(addr).await.map_err(ServerError::Bind)?;
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

    if let Some(snap) = state.snapshot().await {
        // Snapshot exists. Check if it is stale.
        if state.is_snapshot_stale(now).await {
            return health_response_from_state(&state, StatusCode::SERVICE_UNAVAILABLE).await;
        }
        let body = serde_json::to_vec(&*snap).expect("snapshot serializes");
        return (StatusCode::OK, [("content-type", "application/json")], body).into_response();
    }
    health_response_from_state(&state, StatusCode::SERVICE_UNAVAILABLE).await
}

/// GET `/healthz` — returns readiness/health as compact JSON.
///
/// Returns `200` when ready and the snapshot is fresh. Returns `503` when
/// warming, failed, or when the snapshot is stale.
async fn health_handler(State(state): State<ServerState>) -> Response {
    let now = now_unix_ms();

    let status = if state.ready.load(Ordering::Acquire) && !state.is_snapshot_stale(now).await {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    health_response_from_state(&state, status).await
}

/// Any non-matched route returns `404`.
async fn fallback_handler(method: Method, uri: axum::http::Uri) -> (StatusCode, String) {
    (StatusCode::NOT_FOUND, format!("{method} {uri} not found"))
}

async fn health_response_from_state(state: &ServerState, status: StatusCode) -> Response {
    let health = state.health().await;
    let body = serde_json::to_vec(&health).expect("health response serializes");
    (status, [("content-type", "application/json")], body).into_response()
}

#[cfg(test)]
mod tests;
