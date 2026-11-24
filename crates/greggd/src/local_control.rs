//! Typed application-level messages shared by local control transports.
//!
//! Platform transports carry only bounded newline frames; configuration data
//! and paths never cross the control channel.

use tokio::sync::oneshot;

/// Fixed local request; it carries no configuration path or candidate data.
pub(crate) const REHASH_COMMAND: &[u8] = b"REHASH\n";

/// Result produced only after the scheduler has accepted or rejected rehash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RehashResponse {
    Applied,
    Unchanged,
    RestartRequired(Vec<&'static str>),
    Rejected(RehashRejection),
    Busy,
    Uncertain,
}

/// Bounded rejection categories; config contents and parser diagnostics stay local.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RehashRejection {
    InvalidConfig,
    JobAuthority,
    Scheduler,
    ControlUnavailable,
}

/// One scheduler-bound control request with its correlated completion reply.
pub(crate) struct RehashRequest {
    pub(crate) reply: oneshot::Sender<RehashResponse>,
}

/// Client-side classification for a rehash transport that did not return a valid reply.
#[derive(Debug, thiserror::Error)]
pub enum RehashTransportError {
    #[error("greggd is not running for the selected config")]
    NotRunning,
    #[error("rehash outcome is uncertain: {detail}")]
    Uncertain { detail: String },
    #[error("local rehash control failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("local rehash control returned an invalid response")]
    BadResponse,
}

/// Encode a response as one small, newline-terminated ASCII frame.
pub(crate) fn encode_response(response: &RehashResponse) -> Vec<u8> {
    let line = match response {
        RehashResponse::Applied => "APPLIED".to_owned(),
        RehashResponse::Unchanged => "UNCHANGED".to_owned(),
        RehashResponse::RestartRequired(fields) => {
            let fields = fields.join(",");
            format!("RESTART_REQUIRED {fields}")
        }
        RehashResponse::Rejected(reason) => format!(
            "REJECTED {}",
            match reason {
                RehashRejection::InvalidConfig => "invalid_config",
                RehashRejection::JobAuthority => "job_authority",
                RehashRejection::Scheduler => "scheduler",
                RehashRejection::ControlUnavailable => "control_unavailable",
            }
        ),
        RehashResponse::Busy => "BUSY".to_owned(),
        RehashResponse::Uncertain => "UNCERTAIN".to_owned(),
    };
    let mut frame = line.into_bytes();
    frame.push(b'\n');
    frame
}

/// Decode one bounded response frame. Unknown fields/reasons fail closed.
pub(crate) fn decode_response(frame: &[u8]) -> Result<RehashResponse, RehashTransportError> {
    if frame.len() > MAX_REHASH_RESPONSE_BYTES || !frame.ends_with(b"\n") {
        return Err(RehashTransportError::BadResponse);
    }
    let line = std::str::from_utf8(&frame[..frame.len() - 1])
        .map_err(|_| RehashTransportError::BadResponse)?;
    if line.contains(['\n', '\r']) {
        return Err(RehashTransportError::BadResponse);
    }
    match line {
        "APPLIED" => Ok(RehashResponse::Applied),
        "UNCHANGED" => Ok(RehashResponse::Unchanged),
        "BUSY" => Ok(RehashResponse::Busy),
        "UNCERTAIN" => Ok(RehashResponse::Uncertain),
        "RESTART_REQUIRED" => Err(RehashTransportError::BadResponse),
        _ if line.starts_with("RESTART_REQUIRED ") => {
            let fields = &line["RESTART_REQUIRED ".len()..];
            let mut result = Vec::new();
            for field in fields.split(',') {
                let field = match field {
                    "name" => "name",
                    "host" => "host",
                    "port" => "port",
                    "sample_interval_ms" => "sample_interval_ms",
                    "stale_after_ms" => "stale_after_ms",
                    "allow_privileged_jobs" => "allow_privileged_jobs",
                    "scheduler_history_limit" => "scheduler_history_limit",
                    "other" => "other",
                    _ => return Err(RehashTransportError::BadResponse),
                };
                if result.contains(&field) {
                    return Err(RehashTransportError::BadResponse);
                }
                result.push(field);
            }
            if result.is_empty() {
                return Err(RehashTransportError::BadResponse);
            }
            Ok(RehashResponse::RestartRequired(result))
        }
        "REJECTED invalid_config" => Ok(RehashResponse::Rejected(RehashRejection::InvalidConfig)),
        "REJECTED job_authority" => Ok(RehashResponse::Rejected(RehashRejection::JobAuthority)),
        "REJECTED scheduler" => Ok(RehashResponse::Rejected(RehashRejection::Scheduler)),
        "REJECTED control_unavailable" => Ok(RehashResponse::Rejected(
            RehashRejection::ControlUnavailable,
        )),
        _ => Err(RehashTransportError::BadResponse),
    }
}

/// The fixed maximum includes the longest seven-field restart list and newline.
pub(crate) const MAX_REHASH_RESPONSE_BYTES: usize = 160;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_response_round_trips_as_a_bounded_frame() {
        let responses = [
            RehashResponse::Applied,
            RehashResponse::Unchanged,
            RehashResponse::RestartRequired(vec!["host", "port", "sample_interval_ms"]),
            RehashResponse::Rejected(RehashRejection::InvalidConfig),
            RehashResponse::Rejected(RehashRejection::JobAuthority),
            RehashResponse::Rejected(RehashRejection::Scheduler),
            RehashResponse::Busy,
            RehashResponse::Uncertain,
        ];
        for response in responses {
            let encoded = encode_response(&response);
            assert!(encoded.len() <= MAX_REHASH_RESPONSE_BYTES);
            assert_eq!(decode_response(&encoded).unwrap(), response);
        }
    }

    #[test]
    fn malformed_and_unknown_response_frames_are_rejected() {
        for frame in [
            &b"APPLIED"[..],
            &b"APPLIED\nextra\n"[..],
            &b"RESTART_REQUIRED \n"[..],
            &b"RESTART_REQUIRED secret\n"[..],
            &b"REJECTED arbitrary diagnostic\n"[..],
        ] {
            assert!(matches!(
                decode_response(frame),
                Err(RehashTransportError::BadResponse)
            ));
        }
        assert!(matches!(
            decode_response(&[b'x'; MAX_REHASH_RESPONSE_BYTES + 1]),
            Err(RehashTransportError::BadResponse)
        ));
    }
}
