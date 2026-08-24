//! Health and readiness response type.

use serde::{Deserialize, Serialize};

/// Coarse readiness state shared between the daemon and the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessState {
    /// The daemon has a valid cached snapshot and `/v1/status` will return it.
    Ready,
    /// The daemon is alive but the first counter delta is not yet available;
    /// `/v1/status` returns `503`.
    Warming,
    /// The daemon's collector has failed; `/v1/status` returns `503`.
    Failed,
}

/// Machine-readable category for a non-ready health response.
///
/// Categories are deliberately coarse so the client can render consistent
/// diagnostics without leaking implementation details.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthCategory {
    /// Counter delta is still being collected.
    Warming,
    /// The native collector reported an error.
    CollectorFailure,
    /// The daemon is shutting down or otherwise refusing traffic.
    NotServing,
}

/// Health and readiness response served by the daemon.
///
/// The `Ready` variant carries a fresh snapshot. The other variants carry a
/// short human-readable message and a [`HealthCategory`]; they never include
/// filesystem paths, internal error chains, or platform-private structures.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct HealthResponse {
    /// Daemon schema version, always
    /// [`crate::SCHEMA_VERSION_V1`].
    pub schema_version: u16,
    /// Current readiness state.
    pub state: ReadinessState,
    /// Coarse category for non-ready responses. `None` when `state == Ready`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<HealthCategory>,
    /// Short human-readable message. Never includes filesystem paths or
    /// internal error chains.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Cached snapshot, present only when `state == Ready`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<crate::StatusSnapshot>,
}

impl<'de> Deserialize<'de> for HealthResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "snake_case")]
        struct RawHealthResponse {
            schema_version: u16,
            state: ReadinessState,
            #[serde(default)]
            category: Option<HealthCategory>,
            #[serde(default)]
            message: Option<String>,
            #[serde(default)]
            snapshot: Option<crate::StatusSnapshot>,
        }

        let raw = RawHealthResponse::deserialize(deserializer)?;
        if raw.state == ReadinessState::Ready && raw.snapshot.is_none() {
            return Err(serde::de::Error::custom(
                "ready health response must include a snapshot",
            ));
        }
        Ok(Self {
            schema_version: raw.schema_version,
            state: raw.state,
            category: raw.category,
            message: raw.message,
            snapshot: raw.snapshot,
        })
    }
}

impl HealthResponse {
    /// A `Ready` response wrapping the supplied snapshot.
    ///
    /// Callers must validate `snapshot` before constructing a ready response.
    #[must_use]
    pub fn ready(snapshot: crate::StatusSnapshot) -> Self {
        Self {
            schema_version: crate::SCHEMA_VERSION_V1,
            state: ReadinessState::Ready,
            category: None,
            message: None,
            snapshot: Some(snapshot),
        }
    }

    /// A `Warming` response with a default message.
    #[must_use]
    pub fn warming() -> Self {
        Self::warming_with_message("collector warming up")
    }

    /// A `Warming` response with a custom message.
    #[must_use]
    pub fn warming_with_message(message: impl Into<String>) -> Self {
        Self {
            schema_version: crate::SCHEMA_VERSION_V1,
            state: ReadinessState::Warming,
            category: Some(HealthCategory::Warming),
            message: Some(message.into()),
            snapshot: None,
        }
    }

    /// A `Failed` response with the given category and message.
    #[must_use]
    pub fn failed(category: HealthCategory, message: impl Into<String>) -> Self {
        Self {
            schema_version: crate::SCHEMA_VERSION_V1,
            state: ReadinessState::Failed,
            category: Some(category),
            message: Some(message.into()),
            snapshot: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::HealthResponse;

    #[test]
    fn ready_health_requires_snapshot() {
        let json = r#"{"schema_version":1,"state":"ready","snapshot":null}"#;
        assert!(serde_json::from_str::<HealthResponse>(json).is_err());
    }
}
