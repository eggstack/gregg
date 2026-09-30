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

/// Whether `category` is the category a peer is allowed to pair with
/// `state`.
///
/// The mapping is total and one-directional so a wire payload can never
/// carry a contradictory pair:
///
/// - `Ready` never carries a category.
/// - `Warming` carries exactly [`HealthCategory::Warming`]; a "warming"
///   state that also claims a collector failure is incoherent.
/// - `Failed` carries a terminal cause
///   ([`HealthCategory::CollectorFailure`] or
///   [`HealthCategory::NotServing`]); it never re-reports `Warming`.
pub(crate) fn category_allowed(state: ReadinessState, category: Option<HealthCategory>) -> bool {
    match state {
        ReadinessState::Ready => category.is_none(),
        ReadinessState::Warming => category == Some(HealthCategory::Warming),
        ReadinessState::Failed => matches!(
            category,
            Some(HealthCategory::CollectorFailure | HealthCategory::NotServing)
        ),
    }
}

/// Reject a health `message` that is not a short, NUL-free diagnostic.
///
/// Returns the violation reason, or `None` when the message is acceptable.
/// `Ready` responses carry no message at all; this only bounds non-ready
/// messages.
pub(crate) fn message_violation(message: &str) -> Option<&'static str> {
    if message.len() > crate::MAX_HEALTH_MESSAGE_BYTES {
        Some("health message exceeds the protocol maximum")
    } else if message.contains('\0') {
        Some("health message must not contain NUL characters")
    } else {
        None
    }
}

/// Clamp an arbitrary health message into the wire bounds.
///
/// NUL characters are replaced with `�` (U+FFFD) and the result is
/// truncated to [`crate::MAX_HEALTH_MESSAGE_BYTES`] bytes on a UTF-8
/// boundary. Callers that need strict rejection should use the `try_`
/// constructors instead.
pub(crate) fn sanitize_health_message(message: &str) -> String {
    let cleaned = message.replace('\0', "�");
    if cleaned.len() <= crate::MAX_HEALTH_MESSAGE_BYTES {
        return cleaned;
    }
    let mut end = crate::MAX_HEALTH_MESSAGE_BYTES;
    while !cleaned.is_char_boundary(end) {
        end -= 1;
    }
    cleaned[..end].to_owned()
}

/// Health and readiness response served by the daemon.
///
/// The `Ready` variant carries a fresh snapshot. The other variants carry a
/// short human-readable message and a [`HealthCategory`]; they never include
/// filesystem paths, internal error chains, or platform-private structures.
///
/// Do not construct non-Ready responses with struct literals (fields are `pub`
/// for serialization); use `warming()`/`warming_with_message()`/`failed()` so
/// `category`/`message` stay populated. Clients may render a message-less
/// Warming/Failed blank.
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
                if raw.category.is_some() {
                    return Err(serde::de::Error::custom(
                        "ready health response must not include a category",
                    ));
                }
                if raw.message.is_some() {
                    return Err(serde::de::Error::custom(
                        "ready health response must not include a message",
                    ));
                }
            }
            ReadinessState::Failed => {
                if raw.snapshot.is_some() {
                    return Err(serde::de::Error::custom(
                        "non-ready health response must not include a snapshot",
                    ));
                }
                if !category_allowed(raw.state, raw.category) {
                    return Err(serde::de::Error::custom(
                        "failed health response must include a failure category",
                    ));
                }
            }
            ReadinessState::Warming => {
                if raw.snapshot.is_some() {
                    return Err(serde::de::Error::custom(
                        "non-ready health response must not include a snapshot",
                    ));
                }
                if !category_allowed(raw.state, raw.category) {
                    return Err(serde::de::Error::custom(
                        "warming health response must include the warming category",
                    ));
                }
            }
        }
        if let Some(message) = raw.message.as_deref() {
            if let Some(reason) = message_violation(message) {
                return Err(serde::de::Error::custom(reason));
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
    /// Validated input only: the caller must ensure `snapshot` already
    /// passed [`StatusSnapshot::validate`](crate::StatusSnapshot::validate).
    /// Prefer [`Self::try_ready`] when the snapshot comes from an untrusted
    /// source: this constructor asserts the invariant in debug builds and
    /// publishes the snapshot as-is in release builds.
    #[must_use]
    pub fn ready(snapshot: crate::StatusSnapshot) -> Self {
        debug_assert!(
            snapshot.validate().is_ok(),
            "ready health responses require a validated snapshot"
        );
        Self {
            schema_version: crate::SCHEMA_VERSION_V1,
            state: ReadinessState::Ready,
            category: None,
            message: None,
            snapshot: Some(snapshot),
        }
    }

    /// A `Ready` response for a snapshot that is validated first.
    ///
    /// # Errors
    ///
    /// Returns the structured [`ValidationViolation`](crate::ValidationViolation)
    /// list from [`StatusSnapshot::validate`](crate::StatusSnapshot::validate)
    /// so an invalid snapshot can never be advertised as `Ready`.
    pub fn try_ready(
        snapshot: crate::StatusSnapshot,
    ) -> Result<Self, Vec<crate::ValidationViolation>> {
        snapshot.validate()?;
        Ok(Self::ready(snapshot))
    }

    /// A `Warming` response with a default message.
    #[must_use]
    pub fn warming() -> Self {
        Self::warming_with_message("collector warming up")
    }

    /// A `Warming` response with a custom message.
    ///
    /// The message is clamped into the wire bounds (NUL replaced, truncated
    /// to [`crate::MAX_HEALTH_MESSAGE_BYTES`] bytes). Use
    /// [`Self::try_warming_with_message`] to reject invalid messages instead.
    #[must_use]
    pub fn warming_with_message(message: impl Into<String>) -> Self {
        let message: String = message.into();
        let sanitized = sanitize_health_message(&message);
        debug_assert!(
            message_violation(&sanitized).is_none(),
            "sanitized health message must be wire-valid"
        );
        Self {
            schema_version: crate::SCHEMA_VERSION_V1,
            state: ReadinessState::Warming,
            category: Some(HealthCategory::Warming),
            message: Some(sanitized),
            snapshot: None,
        }
    }

    /// A `Warming` response with a custom message, rejected when invalid.
    ///
    /// # Errors
    ///
    /// Returns the wire violation reason when the message exceeds
    /// [`crate::MAX_HEALTH_MESSAGE_BYTES`] bytes or contains NUL.
    pub fn try_warming_with_message(message: impl Into<String>) -> Result<Self, &'static str> {
        let message = message.into();
        if let Some(reason) = message_violation(&message) {
            return Err(reason);
        }
        Ok(Self {
            schema_version: crate::SCHEMA_VERSION_V1,
            state: ReadinessState::Warming,
            category: Some(HealthCategory::Warming),
            message: Some(message),
            snapshot: None,
        })
    }

    /// A `Failed` response with the given category and message.
    ///
    /// The message is clamped into the wire bounds (NUL replaced, truncated
    /// to [`crate::MAX_HEALTH_MESSAGE_BYTES`] bytes). Use
    /// [`Self::try_failed`] to reject invalid messages instead.
    #[must_use]
    pub fn failed(category: HealthCategory, message: impl Into<String>) -> Self {
        let message: String = message.into();
        let sanitized = sanitize_health_message(&message);
        debug_assert!(
            message_violation(&sanitized).is_none(),
            "sanitized health message must be wire-valid"
        );
        Self {
            schema_version: crate::SCHEMA_VERSION_V1,
            state: ReadinessState::Failed,
            category: Some(category),
            message: Some(sanitized),
            snapshot: None,
        }
    }

    /// A `Failed` response with the given category and message, rejected when invalid.
    ///
    /// # Errors
    ///
    /// Returns the wire violation reason when the message exceeds
    /// [`crate::MAX_HEALTH_MESSAGE_BYTES`] bytes or contains NUL.
    pub fn try_failed(
        category: HealthCategory,
        message: impl Into<String>,
    ) -> Result<Self, &'static str> {
        let message = message.into();
        if let Some(reason) = message_violation(&message) {
            return Err(reason);
        }
        Ok(Self {
            schema_version: crate::SCHEMA_VERSION_V1,
            state: ReadinessState::Failed,
            category: Some(category),
            message: Some(message),
            snapshot: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{HealthCategory, HealthResponse, ReadinessState};
    use crate::MAX_HEALTH_MESSAGE_BYTES;

    const READY_SNAPSHOT: &str = r#"{"schema_version":1,"observed_at_unix_ms":1,"sample_interval_ms":1000,"capabilities":{"cpu_iowait":false},"system":{"name":"n","hostname":"h","os_name":"linux","os_version":"1","kernel_name":"Linux","kernel_release":"6","architecture":"x86_64"},"cpu":{"logical_cores":1,"usage_pct":0.0,"iowait_pct":null},"load":{"one":0.0,"five":0.0,"fifteen":0.0},"memory":{"used_bytes":0,"total_bytes":1,"usage_pct":0.0},"swap":{"used_bytes":0,"total_bytes":1,"usage_pct":0.0}}"#;

    fn ready_with(extra: &str) -> String {
        format!(r#"{{"schema_version":1,"state":"ready","snapshot":{READY_SNAPSHOT}{extra}}}"#)
    }

    #[test]
    fn ready_health_requires_snapshot() {
        let json = r#"{"schema_version":1,"state":"ready","snapshot":null}"#;
        assert!(serde_json::from_str::<HealthResponse>(json).is_err());
    }

    #[test]
    fn ready_health_rejects_category() {
        let json = r#"{"schema_version":1,"state":"ready","category":"warming","snapshot":{"schema_version":1,"observed_at_unix_ms":1,"sample_interval_ms":1000,"capabilities":{"cpu_iowait":false},"system":{"name":"n","hostname":"h","os_name":"linux","os_version":"1","kernel_name":"Linux","kernel_release":"6","architecture":"x86_64"},"cpu":{"logical_cores":1,"usage_pct":0.0,"iowait_pct":null},"load":{"one":0.0,"five":0.0,"fifteen":0.0},"memory":{"used_bytes":0,"total_bytes":1,"usage_pct":0.0},"swap":{"used_bytes":0,"total_bytes":1,"usage_pct":0.0}}}"#;
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

    #[test]
    fn non_ready_states_reject_contradictory_categories() {
        for (state, category) in [
            ("warming", "collector_failure"),
            ("warming", "not_serving"),
            ("failed", "warming"),
        ] {
            let json = format!(
                r#"{{"schema_version":1,"state":"{state}","category":"{category}","message":"x"}}"#
            );
            assert!(
                serde_json::from_str::<HealthResponse>(&json).is_err(),
                "{state} + {category} must be rejected"
            );
        }
        for (state, category) in [
            ("warming", "warming"),
            ("failed", "collector_failure"),
            ("failed", "not_serving"),
        ] {
            let json = format!(
                r#"{{"schema_version":1,"state":"{state}","category":"{category}","message":"x"}}"#
            );
            let parsed: HealthResponse = serde_json::from_str(&json).expect("valid pairing");
            assert_eq!(
                parsed.category,
                Some(match category {
                    "warming" => HealthCategory::Warming,
                    "collector_failure" => HealthCategory::CollectorFailure,
                    _ => HealthCategory::NotServing,
                })
            );
        }
    }

    #[test]
    fn ready_health_rejects_a_message() {
        let json = ready_with(r#","message":"hi""#);
        assert!(serde_json::from_str::<HealthResponse>(&json).is_err());
    }

    #[test]
    fn health_messages_are_bounded_and_nul_free() {
        // Wire still rejects oversize/NUL payloads from untrusted peers.
        let oversize = "x".repeat(MAX_HEALTH_MESSAGE_BYTES + 1);
        let raw = serde_json::json!({
            "schema_version": 1,
            "state": "failed",
            "category": "collector_failure",
            "message": oversize,
        });
        assert!(serde_json::from_value::<HealthResponse>(raw).is_err());
        assert!(HealthResponse::try_failed(HealthCategory::CollectorFailure, &oversize).is_err());
        assert!(HealthResponse::try_warming_with_message(&oversize).is_err());

        let json = r#"{"schema_version":1,"state":"failed","category":"collector_failure","message":"a\u0000b"}"#;
        assert!(serde_json::from_str::<HealthResponse>(json).is_err());
        assert!(HealthResponse::try_failed(HealthCategory::CollectorFailure, "a\0b").is_err());

        // Clamping constructors stay wire-valid.
        let clamped = HealthResponse::failed(HealthCategory::CollectorFailure, &oversize);
        let message = clamped.message.as_deref().expect("message");
        assert_eq!(message.len(), MAX_HEALTH_MESSAGE_BYTES);
        let json = serde_json::to_string(&clamped).expect("serialize");
        assert!(serde_json::from_str::<HealthResponse>(&json).is_ok());

        let nul_clamped = HealthResponse::failed(HealthCategory::CollectorFailure, "a\0b");
        let message = nul_clamped.message.as_deref().expect("message");
        assert!(!message.contains('\0'));
        let json = serde_json::to_string(&nul_clamped).expect("serialize");
        assert!(serde_json::from_str::<HealthResponse>(&json).is_ok());

        let at_bound = "x".repeat(MAX_HEALTH_MESSAGE_BYTES);
        let json = serde_json::to_string(&HealthResponse::warming_with_message(at_bound))
            .expect("serialize");
        let parsed: HealthResponse = serde_json::from_str(&json).expect("at-bound message parses");
        assert_eq!(parsed.state, ReadinessState::Warming);
    }

    #[test]
    fn try_ready_refuses_an_invalid_snapshot() {
        let snapshot: crate::StatusSnapshot = serde_json::from_str(READY_SNAPSHOT).expect("parse");
        HealthResponse::try_ready(snapshot.clone()).expect("valid snapshot is ready");

        let mut invalid = snapshot;
        invalid.memory.total_bytes = 0;
        invalid.memory.used_bytes = 5;
        assert!(HealthResponse::try_ready(invalid).is_err());
    }
}
