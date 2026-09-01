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

/// Deserialization checks envelope invariants (schema version, snapshot
/// presence per readiness state) but deliberately does **not** validate the
/// embedded snapshot. Callers must invoke
/// [`StatusSnapshot::validate`](crate::StatusSnapshot::validate) on the
/// received snapshot themselves.
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
        if raw.schema_version != crate::SCHEMA_VERSION_V1 {
            return Err(serde::de::Error::custom(format!(
                "unsupported schema_version {} (expected {})",
                raw.schema_version,
                crate::SCHEMA_VERSION_V1
            )));
        }
        match raw.state {
            ReadinessState::Ready => {
                if raw.snapshot.is_none() {
                    return Err(serde::de::Error::custom(
                        "ready health response must include a snapshot",
                    ));
                }
            }
            ReadinessState::Failed => {
                if raw.snapshot.is_some() {
                    return Err(serde::de::Error::custom(
                        "non-ready health response must not include a snapshot",
                    ));
                }
                if raw.category.is_none() {
                    return Err(serde::de::Error::custom(
                        "failed health response must include a category",
                    ));
                }
            }
            ReadinessState::Warming => {
                if raw.snapshot.is_some() {
                    return Err(serde::de::Error::custom(
                        "non-ready health response must not include a snapshot",
                    ));
                }
                if raw.category.is_none() {
                    return Err(serde::de::Error::custom(
                        "warming health response must include a category",
                    ));
                }
            }
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

    #[test]
    fn health_rejects_unsupported_schema_version() {
        let json = r#"{"schema_version":99,"state":"warming"}"#;
        assert!(serde_json::from_str::<HealthResponse>(json).is_err());
    }

    #[test]
    fn non_ready_health_forbids_snapshot() {
        let json = r#"{"schema_version":1,"state":"warming","snapshot":{"schema_version":1,"observed_at_unix_ms":1,"sample_interval_ms":1000,"capabilities":{"cpu_iowait":false},"system":{"name":"n","hostname":"h","os_name":"linux","os_version":"1","kernel_name":"Linux","kernel_release":"6","architecture":"x86_64"},"cpu":{"logical_cores":1,"usage_pct":0.0,"iowait_pct":null},"load":{"one":0.0,"five":0.0,"fifteen":0.0},"memory":{"used_bytes":0,"total_bytes":1,"usage_pct":0.0},"swap":{"used_bytes":0,"total_bytes":1,"usage_pct":0.0}}}"#;
        assert!(serde_json::from_str::<HealthResponse>(json).is_err());
    }

    #[test]
    fn failed_health_requires_category() {
        let json = r#"{"schema_version":1,"state":"failed","message":"collector failed"}"#;
        assert!(serde_json::from_str::<HealthResponse>(json).is_err());
    }

    #[test]
    fn warming_health_requires_category() {
        let json = r#"{"schema_version":1,"state":"warming","message":"warming"}"#;
        assert!(serde_json::from_str::<HealthResponse>(json).is_err());
    }
}
