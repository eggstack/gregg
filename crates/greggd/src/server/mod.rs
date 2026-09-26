//! HTTP server for the `greggd` daemon.
//!
//! Exposes read-only status and health endpoints:
//!
//! - `GET /` and `GET /v1/status` — latest v1 status snapshot as compact JSON.
//! - `GET /v2/status` — latest flat v2 status payload, including optional drives.
//! - `GET /healthz` — readiness and health information.
//!
//! Unsupported methods on a known path return `405`; unknown paths return
//! `404`. No TLS, cookies, sessions,
//! multipart handling, WebSocket upgrade, compression, or static-file serving
//! is supported.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use eggserve_primitives::canonical::{Response, ResponseBody, ResponseStream, StatusCode};
use eggserve_primitives::request::Request;
use eggserve_primitives::request_body_policy::RequestBodyPolicy;
use eggserve_server::{
    service_fn_with_policy, RuntimeConfig, Server, ServerCompletion, ServerControl, Service,
    ServiceError,
};
use gregg_protocol::v2::SCHEMA_VERSION_V2;
use gregg_protocol::v2::{HealthResponseV2, StatusPayloadV2, StatusSnapshotV2};
use gregg_protocol::{HealthResponse, ReadinessState, StatusSnapshot, SCHEMA_VERSION_V1};
use tokio::net::TcpListener;
use tokio::sync::{RwLock, Semaphore};
use tracing::info;

use crate::server::error::{ServerConfigError, ServerError};

pub mod error;

const V1_UNAVAILABLE_MESSAGE: &str = "schema v1 status is unavailable on this platform";
const MAX_REQUEST_BODY_BYTES: u64 = 64 * 1024;

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
    #[cfg(test)]
    v1_health_serializations: Arc<std::sync::atomic::AtomicUsize>,
    #[cfg(test)]
    v2_health_serializations: Arc<std::sync::atomic::AtomicUsize>,
    #[cfg(test)]
    fallback_bodies_built: Arc<std::sync::atomic::AtomicUsize>,
}

#[derive(Debug)]
struct PublishedState {
    snapshot: Option<Arc<StatusSnapshot>>,
    snapshot_v2: Option<Arc<StatusPayloadV2>>,
    status_bytes: Option<Bytes>,
    status_bytes_v2: Option<Bytes>,
    /// Plan 139: ready-health bodies memoized per immutable publication.
    /// Valid only while `health`/`health_v2` remains `Ready` and the snapshot
    /// is fresh at request time. Cleared on every new publication, warming,
    /// or failure transition so stale/failed responses stay dynamic.
    health_bytes: Option<Bytes>,
    health_bytes_v2: Option<Bytes>,
    last_observed_at_unix_ms: Option<u64>,
    health: HealthMetadata,
    health_v2: HealthMetadata,
    consecutive_failures: u32,
}

/// Plan 139: borrowed ready-health serialization view for v1.
///
/// Byte-for-byte equivalent to `HealthResponse::ready(snapshot.clone())`
/// without deep-cloning the snapshot. Field order and `skip_serializing_if`
/// attributes mirror [`HealthResponse`] exactly.
#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct BorrowedReadyHealthV1<'a> {
    schema_version: u16,
    state: ReadinessState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    category: Option<gregg_protocol::HealthCategory>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    snapshot: Option<&'a StatusSnapshot>,
}

/// Plan 139: borrowed ready-health serialization view for v2.
///
/// Byte-for-byte equivalent to `HealthResponseV2::ready(snapshot.clone())`.
#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct BorrowedReadyHealthV2<'a> {
    schema_version: u16,
    state: ReadinessState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    category: Option<gregg_protocol::HealthCategory>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    snapshot: Option<&'a StatusSnapshotV2>,
}

fn serialize_ready_health_borrowed(snapshot: &StatusSnapshot) -> Result<Bytes, serde_json::Error> {
    let view = BorrowedReadyHealthV1 {
        schema_version: SCHEMA_VERSION_V1,
        state: ReadinessState::Ready,
        category: None,
        message: None,
        snapshot: Some(snapshot),
    };
    serde_json::to_vec(&view).map(Bytes::from)
}

fn serialize_ready_health_v2_borrowed(
    snapshot: &StatusSnapshotV2,
) -> Result<Bytes, serde_json::Error> {
    let view = BorrowedReadyHealthV2 {
        schema_version: SCHEMA_VERSION_V2,
        state: ReadinessState::Ready,
        category: None,
        message: None,
        snapshot: Some(snapshot),
    };
    serde_json::to_vec(&view).map(Bytes::from)
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

    fn stale_v1_response(&self) -> HealthResponse {
        if self.state == ReadinessState::Ready {
            HealthResponse::failed(
                gregg_protocol::HealthCategory::CollectorFailure,
                "cached snapshot is stale",
            )
        } else {
            self.v1_response(None)
        }
    }

    fn stale_v2_response(&self) -> HealthResponseV2 {
        if self.state == ReadinessState::Ready {
            HealthResponseV2::failed(
                gregg_protocol::HealthCategory::CollectorFailure,
                "cached snapshot is stale",
            )
        } else {
            self.v2_response(None)
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
                health_bytes: None,
                health_bytes_v2: None,
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
            #[cfg(test)]
            v1_health_serializations: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            #[cfg(test)]
            v2_health_serializations: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            #[cfg(test)]
            fallback_bodies_built: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
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

    #[allow(clippy::unused_self)]
    fn serialize_v1_ready_health(
        &self,
        snapshot: &StatusSnapshot,
    ) -> Result<Bytes, serde_json::Error> {
        #[cfg(test)]
        self.v1_health_serializations
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        serialize_ready_health_borrowed(snapshot)
    }

    #[allow(clippy::unused_self)]
    fn serialize_v2_ready_health(
        &self,
        snapshot: &StatusSnapshotV2,
    ) -> Result<Bytes, serde_json::Error> {
        #[cfg(test)]
        self.v2_health_serializations
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        serialize_ready_health_v2_borrowed(snapshot)
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
        state.health_bytes = None;
        state.health_bytes_v2 = None;
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
        state.health_bytes = None;
        state.health_bytes_v2 = None;
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
        state.health_bytes = None;
        state.health_bytes_v2 = None;
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
        state.health_bytes = None;
        state.health_bytes_v2 = None;
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
        // Ready-health memos must not survive a failure transition; the
        // next ready publication rebuilds them. NotServing memos are never
        // cached, so clearing unconditionally is exact.
        state.health_bytes = None;
        state.health_bytes_v2 = None;
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
                return StatusDataV1::Unavailable(Box::new(state.health.stale_v1_response()));
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
                return StatusDataV2::Unavailable(Box::new(state.health_v2.stale_v2_response()));
            }
            return state.status_bytes_v2.as_ref().map_or_else(
                || StatusDataV2::FreshTyped(Arc::clone(snapshot)),
                |body| StatusDataV2::FreshCached(body.clone()),
            );
        }
        StatusDataV2::Unavailable(Box::new(state.health_v2.v2_response(None)))
    }

    /// Plan 139: ready-health fast path returning cached bytes when the
    /// publication is Ready and fresh, otherwise a dynamic body.
    ///
    /// Returns the body plus whether it was served from the per-publication
    /// memo (used only by tests via serialization counters).
    async fn v1_health_cached(
        &self,
        now_unix_ms: Option<u64>,
    ) -> Result<(Bytes, StatusCode), ServiceError> {
        let (snapshot, cached, state_is_ready, snapshot_is_stale) = {
            let published = self.published.read().await;
            let snapshot_stale = self.is_stale(&published, now_unix_ms);
            let ready = published.health.state == ReadinessState::Ready;
            if snapshot_stale && ready {
                let body = serialize_health(&HealthResponse::failed(
                    gregg_protocol::HealthCategory::CollectorFailure,
                    "cached snapshot is stale",
                ))?;
                return Ok((body, StatusCode::SERVICE_UNAVAILABLE));
            }
            if !ready {
                let health = published.health.v1_response(published.snapshot.as_deref());
                let status = if health.state == ReadinessState::Ready && !snapshot_stale {
                    StatusCode::OK
                } else {
                    StatusCode::SERVICE_UNAVAILABLE
                };
                let body = serialize_health(&health)?;
                return Ok((body, status));
            }
            let Some(snapshot) = published.snapshot.clone() else {
                let health = published.health.v1_response(None);
                let body = serialize_health(&health)?;
                return Ok((body, StatusCode::SERVICE_UNAVAILABLE));
            };
            (
                snapshot,
                published.health_bytes.clone(),
                ready,
                snapshot_stale,
            )
        };
        debug_assert!(state_is_ready && !snapshot_is_stale);
        if let Some(body) = cached {
            return Ok((body, StatusCode::OK));
        }
        let body = self
            .serialize_v1_ready_health(&snapshot)
            .map_err(|error| ServiceError::internal(error.to_string()))?;
        // Publish the memo only if the same immutable snapshot is still
        // current and still Ready; otherwise serve without polluting the
        // newer generation.
        {
            let mut state = self.published.write().await;
            let still_current = state
                .snapshot
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &snapshot))
                && state.health.state == ReadinessState::Ready;
            if still_current && state.health_bytes.is_none() {
                state.health_bytes = Some(body.clone());
            }
        }
        Ok((body, StatusCode::OK))
    }

    /// Plan 139: v2 ready-health fast path, mirroring [`Self::v1_health_cached`].
    async fn v2_health_cached(
        &self,
        now_unix_ms: Option<u64>,
    ) -> Result<(Bytes, StatusCode), ServiceError> {
        let (snapshot_v2, cached, state_is_ready, snapshot_is_stale) = {
            let published = self.published.read().await;
            let snapshot_stale = self.is_stale(&published, now_unix_ms);
            let ready = published.health_v2.state == ReadinessState::Ready;
            if snapshot_stale && ready {
                let body = serialize_health_v2(&HealthResponseV2::failed(
                    gregg_protocol::HealthCategory::CollectorFailure,
                    "cached snapshot is stale",
                ))?;
                return Ok((body, StatusCode::SERVICE_UNAVAILABLE));
            }
            if !ready {
                let health = published.health_v2.v2_response(
                    published
                        .snapshot_v2
                        .as_deref()
                        .map(|payload| &payload.snapshot),
                );
                let status = if health.state == ReadinessState::Ready && !snapshot_stale {
                    StatusCode::OK
                } else {
                    StatusCode::SERVICE_UNAVAILABLE
                };
                let body = serialize_health_v2(&health)?;
                return Ok((body, status));
            }
            let Some(snapshot_v2) = published.snapshot_v2.clone() else {
                let health = published.health_v2.v2_response(None);
                let body = serialize_health_v2(&health)?;
                return Ok((body, StatusCode::SERVICE_UNAVAILABLE));
            };
            (
                snapshot_v2,
                published.health_bytes_v2.clone(),
                ready,
                snapshot_stale,
            )
        };
        debug_assert!(state_is_ready && !snapshot_is_stale);
        if let Some(body) = cached {
            return Ok((body, StatusCode::OK));
        }
        let body = self
            .serialize_v2_ready_health(&snapshot_v2.snapshot)
            .map_err(|error| ServiceError::internal(error.to_string()))?;
        {
            let mut state = self.published.write().await;
            let still_current = state
                .snapshot_v2
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &snapshot_v2))
                && state.health_v2.state == ReadinessState::Ready;
            if still_current && state.health_bytes_v2.is_none() {
                state.health_bytes_v2 = Some(body.clone());
            }
        }
        Ok((body, StatusCode::OK))
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

/// Start `EggServe` on the listener already bound by Gregg.
///
/// The split control/completion pair lets the daemon supervisor observe
/// terminal errors while retaining a separate graceful-shutdown capability.
pub(crate) async fn start(
    listener: TcpListener,
    state: ServerState,
) -> Result<(ServerControl, ServerCompletion), ServerError> {
    let addr = listener.local_addr().map_err(ServerError::Bind)?;
    let config =
        runtime_config().map_err(|error| ServerError::Runtime(std::io::Error::other(error)))?;
    let server = Server::builder()
        .runtime(config)
        .from_listener(listener)
        .build()
        .map_err(|error| ServerError::Runtime(std::io::Error::other(error)))?;
    let handle = server
        .start_with_service(http_service(state))
        .await
        .map_err(|error| ServerError::Runtime(std::io::Error::other(error)))?;
    let (control, completion) = handle.into_parts();
    info!("greggd listening on {addr}");
    Ok((control, completion))
}

/// `EggServe` limits are selected explicitly so a future default change cannot
/// silently change Gregg's wire or lifecycle contract.
fn runtime_config() -> Result<RuntimeConfig, eggserve_server::ServerError> {
    RuntimeConfig::builder()
        // Preserve practical parity with Hyper's unbounded accepted
        // connection/request concurrency; this is the semaphore's maximum,
        // not EggServe's small default of 64.
        .max_connections(Semaphore::MAX_PERMITS)
        .max_in_flight_requests(Semaphore::MAX_PERMITS)
        // Hyper's H1 parser defaults to 100 fields and a 417,792-byte read
        // buffer. EggServe applies these bounds explicitly.
        .max_headers(100)
        .max_buf_size(417_792)
        .max_header_bytes(417_792)
        .max_request_target_bytes(65_536)
        // Gregg accepts and ignores ordinary GET bodies today. Keep that
        // behavior for bodies up to a bounded 64 KiB ceiling.
        .max_request_body_bytes(MAX_REQUEST_BODY_BYTES)
        // Match Axum/Hyper's observed Date header and lack of Server header.
        .response_policy(eggserve_server::response_policy::ResponsePolicy::standard())
        .header_read_timeout(Duration::from_secs(10))
        .handler_timeout(Duration::from_secs(30))
        .body_read_timeout(Duration::from_secs(30))
        .keep_alive_idle_timeout(Duration::from_secs(60))
        .disable_connection_total_timeout()
        .max_requests_per_connection(None)
        .response_write_timeout(Duration::from_secs(30))
        // Gregg's outer 10-second cleanup deadline remains authoritative.
        .graceful_shutdown_timeout(Duration::from_secs(8))
        .build()
}

fn http_service(state: ServerState) -> impl Service {
    service_fn_with_policy(
        move |request: Request| {
            let state = state.clone();
            async move { dispatch_request(&state, request).await }
        },
        RequestBodyPolicy::Buffer {
            max_bytes: MAX_REQUEST_BODY_BYTES,
        },
    )
}

async fn dispatch_request(state: &ServerState, request: Request) -> Result<Response, ServiceError> {
    let (head, _body) = request.into_head_and_body();
    // Plan 139: keep method/target borrowed through route selection so
    // successful known routes never allocate owned dispatch strings. Only
    // the 404 fallback builds its text body from owned copies.
    let method = head.method().as_str();
    let path = head.target().path();
    let known_route = matches!(
        path,
        "/" | "/v1/status" | "/v2/status" | "/healthz" | "/v2/healthz"
    );

    if known_route && !matches!(method, "GET" | "HEAD") {
        return method_not_allowed_response();
    }

    match (method, path) {
        ("GET" | "HEAD", "/" | "/v1/status") => v1_status_response(state).await,
        ("GET" | "HEAD", "/v2/status") => v2_status_response(state).await,
        ("GET" | "HEAD", "/healthz") => health_response(state).await,
        ("GET" | "HEAD", "/v2/healthz") => health_response_v2(state).await,
        _ => {
            let method_owned = method.to_owned();
            let raw_owned = head.target().raw().to_owned();
            #[cfg(test)]
            state
                .fallback_bodies_built
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            not_found_response(&method_owned, &raw_owned)
        }
    }
}

async fn v1_status_response(state: &ServerState) -> Result<Response, ServiceError> {
    match state.v1_status_data(now_unix_ms()).await {
        StatusDataV1::FreshCached(body) => json_response(StatusCode::OK, body),
        StatusDataV1::FreshTyped(snapshot) => match state.serialize_v1_status(&snapshot) {
            Ok(body) => json_response(StatusCode::OK, body),
            Err(error) => serialization_error_response(&error),
        },
        StatusDataV1::Unavailable(health) => {
            json_response(StatusCode::SERVICE_UNAVAILABLE, serialize_health(&health)?)
        }
    }
}

async fn v2_status_response(state: &ServerState) -> Result<Response, ServiceError> {
    match state.v2_status_data(now_unix_ms()).await {
        StatusDataV2::FreshCached(body) => json_response(StatusCode::OK, body),
        StatusDataV2::FreshTyped(snapshot) => match state.serialize_v2_status(&snapshot) {
            Ok(body) => json_response(StatusCode::OK, body),
            Err(error) => serialization_error_response(&error),
        },
        StatusDataV2::Unavailable(health) => json_response(
            StatusCode::SERVICE_UNAVAILABLE,
            serialize_health_v2(&health)?,
        ),
    }
}

async fn health_response(state: &ServerState) -> Result<Response, ServiceError> {
    let (body, status) = state.v1_health_cached(now_unix_ms()).await?;
    json_response(status, body)
}

async fn health_response_v2(state: &ServerState) -> Result<Response, ServiceError> {
    let (body, status) = state.v2_health_cached(now_unix_ms()).await?;
    json_response(status, body)
}

fn json_response(status: StatusCode, body: Bytes) -> Result<Response, ServiceError> {
    let length = u64::try_from(body.len())
        .map_err(|error| ServiceError::internal(format!("status body length: {error}")))?;
    let stream = ResponseStream::with_known_length(
        futures_util::stream::iter([Ok::<_, eggserve_primitives::ResponseStreamError>(body)]),
        length,
    );
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .map_err(|error| ServiceError::internal(error.to_string()))?
        .body(ResponseBody::Stream(stream))
        .map_err(|error| ServiceError::internal(error.to_string()))
}

fn method_not_allowed_response() -> Result<Response, ServiceError> {
    Response::builder()
        .status(StatusCode::METHOD_NOT_ALLOWED)
        .header("allow", "GET,HEAD")
        .map_err(|error| ServiceError::internal(error.to_string()))?
        .empty()
        .map_err(|error| ServiceError::internal(error.to_string()))
}

fn not_found_response(method: &str, target: &str) -> Result<Response, ServiceError> {
    let body = Bytes::from(format!("{method} {target} not found"));
    let length = u64::try_from(body.len())
        .map_err(|error| ServiceError::internal(format!("fallback body length: {error}")))?;
    let stream = ResponseStream::with_known_length(
        futures_util::stream::iter([Ok::<_, eggserve_primitives::ResponseStreamError>(body)]),
        length,
    );
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header("content-type", "text/plain; charset=utf-8")
        .map_err(|error| ServiceError::internal(error.to_string()))?
        .body(ResponseBody::Stream(stream))
        .map_err(|error| ServiceError::internal(error.to_string()))
}

fn serialization_error_response(error: &serde_json::Error) -> Result<Response, ServiceError> {
    let error_body = serde_json::to_vec(&serde_json::json!({"error": error.to_string()}))
        .unwrap_or_else(|_| b"{\"error\":\"serialization failed\"}".to_vec());
    json_response(StatusCode::INTERNAL_SERVER_ERROR, Bytes::from(error_body))
}

fn serialize_health(health: &HealthResponse) -> Result<Bytes, ServiceError> {
    serde_json::to_vec(health)
        .map(Bytes::from)
        .map_err(|error| ServiceError::internal(error.to_string()))
}

fn serialize_health_v2(health: &HealthResponseV2) -> Result<Bytes, ServiceError> {
    serde_json::to_vec(health)
        .map(Bytes::from)
        .map_err(|error| ServiceError::internal(error.to_string()))
}

#[cfg(test)]
mod tests;
