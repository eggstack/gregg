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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
            port: 11310,
            sample_interval_ms: 1000,
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
    /// Latest status snapshot. `None` during warmup.
    snapshot: Arc<RwLock<Option<Arc<StatusSnapshot>>>>,
    /// Readiness flag: `true` once a valid snapshot is available.
    ready: Arc<AtomicBool>,
    /// Current health response.
    health: Arc<RwLock<HealthResponse>>,
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
        Self {
            snapshot: Arc::new(RwLock::new(None)),
            ready: Arc::new(AtomicBool::new(false)),
            health: Arc::new(RwLock::new(HealthResponse::warming())),
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
        self.ready.store(true, Ordering::Release);
    }

    /// Set the daemon to warming state.
    pub async fn set_warming(&self) {
        self.ready.store(false, Ordering::Release);
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
    pub async fn set_failed(&self, msg: &str) {
        self.ready.store(false, Ordering::Release);
        {
            let mut guard = self.snapshot.write().await;
            *guard = None;
        }
        {
            let mut guard = self.health.write().await;
            *guard = HealthResponse::failed(gregg_protocol::HealthCategory::CollectorFailure, msg);
        }
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
async fn status_handler(State(state): State<ServerState>) -> Response {
    if state.ready.load(Ordering::Acquire) {
        if let Some(snap) = state.snapshot().await {
            let body = serde_json::to_vec(&*snap).expect("snapshot serializes");
            return (StatusCode::OK, [("content-type", "application/json")], body).into_response();
        }
    }
    health_response_from_state(&state, StatusCode::SERVICE_UNAVAILABLE).await
}

/// GET `/healthz` — returns readiness/health as compact JSON.
async fn health_handler(State(state): State<ServerState>) -> Response {
    let status = if state.ready.load(Ordering::Acquire) {
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
