//! Plan 164: the local client-daemon IPC protocol.
//!
//! A length-prefixed, newline-delimited JSON channel between a `gregg` TUI
//! frontend and the per-config client daemon. These DTOs are **deliberately not
//! in `gregg-protocol`**: that crate is the cross-version remote wire contract
//! shared with every daemin in a fleet, while this is a same-binary, same-host,
//! local-only channel between two halves of one `gregg` install. Putting it in
//! the protocol crate would imply a compatibility obligation the local channel
//! does not have — the version handshake exists precisely so a mismatched pair
//! can be *detected and refused* rather than forward-compatible.
//!
//! # Versioned handshake
//!
//! [`PROTOCOL_VERSION`] must match on both ends. A mismatch is reported as an
//! explicit [`FrontendFrame::VersionMismatch`] and the frontend exits with
//! actionable guidance. The daemon is **not** asked to serve an older protocol
//! or to translate frames: that would keep a stale TUI alive against a daemon
//! it cannot correctly drive, which is exactly the failure the handshake exists
//! to prevent.
//!
//! # Bounded frames
//!
//! Every frame is bounded by [`MAX_FRAME_BYTES`] and rejected with
//! [`FrontendFrame::ProtocolError`] before any allocation grows past it, so a
//! confused or hostile peer cannot exhaust memory. The bound is a hard
//! invariant of the channel, not an advisory limit.
//!
//! # Latest state, not an event log
//!
//! The daemon pushes [`FrontendFrame::Snapshot`] documents, each complete and
//! self-contained. A slow client **skips** intermediate snapshots rather than
//! queueing them: only the newest state is ever interesting, so backpressure
//! is resolved by dropping stale frames, never by delaying the daemon. This is
//! the direct opposite of the metrics `PollBatch` channel, which must not lose
//! an ordered result.
//!
//! The one non-latest-state message is [`FrontendFrame::ControlAck`], which
//! acknowledges a specific request generation. A frontend that wants a definite
//! answer to a command matches the generation; it never infers completion from
//! the absence of new state.

use serde::{Deserialize, Serialize};

use crate::clientd::snapshot;

/// Local IPC protocol version. Bump on any breaking frame change.
pub const PROTOCOL_VERSION: u16 = 1;

/// Hard cap on one encoded frame, in bytes.
///
/// Sized above the largest realistic snapshot (64 systems plus a bounded cron
/// history) and far below anything that could exhaust memory.
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

/// Startup grace before the daemon is considered not-yet-ready.
pub const DEFAULT_LAUNCH_TIMEOUT_SECS: u64 = 20;

/// A state document the daemon pushes to a connected frontend.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum FrontendFrame {
    /// Full, self-contained state. Always complete: a client can render from
    /// this alone.
    Snapshot(Box<snapshot::FrontendSnapshot>),
    /// Daemon identity and protocol version, sent once immediately after a
    /// successful handshake.
    Hello(Box<HelloPayload>),
    /// The daemon's protocol version differs from the frontend's. The frontend
    /// must exit rather than render frames it cannot interpret.
    VersionMismatch(VersionMismatchPayload),
    /// The request was refused; no state changed.
    ProtocolError(String),
    /// Acknowledgement of a control request, matched by generation.
    ControlAck {
        /// Generation of the request being acknowledged.
        generation: u64,
        /// Whether the request was applied.
        accepted: bool,
        /// Human-readable reason when `accepted` is false.
        detail: Option<String>,
    },
    /// The daemon is shutting down and will close the connection. A frontend
    /// should report this rather than treating it as a transport failure.
    ShuttingDown,
}

/// Daemon identity, sent once after a successful handshake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloPayload {
    /// Local protocol version. Must equal [`PROTOCOL_VERSION`].
    pub protocol_version: u16,
    /// Workspace version of the daemon binary.
    pub version: String,
    /// Config-specific identity digest.
    pub daemon_id: String,
    /// Monotonic generation of the first snapshot this daemon will send.
    pub current_generation: u64,
}

/// Reported when the two ends disagree on [`PROTOCOL_VERSION`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionMismatchPayload {
    /// Version the frontend speaks.
    pub frontend: u16,
    /// Version the daemon speaks.
    pub daemon: u16,
}

/// Commands a frontend may send.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum DaemonRequest {
    /// Versioned handshake. Must be the first frame on a connection.
    Handshake {
        /// Protocol version the frontend speaks.
        protocol_version: u16,
        /// Workspace version of the frontend binary.
        version: String,
    },
    /// Re-read the configuration file. This is the only reload boundary.
    ReloadConfig {
        /// Monotonic generation chosen by the frontend, echoed in the ack.
        generation: u64,
    },
    /// Ask the daemon to stop; used by `gregg daemon stop`.
    Shutdown {
        /// Monotonic generation chosen by the caller, echoed in the ack.
        generation: u64,
    },
}

impl DaemonRequest {
    /// The generation this request will be acknowledged with, if it has one.
    #[must_use]
    pub fn generation(&self) -> Option<u64> {
        match self {
            Self::ReloadConfig { generation } | Self::Shutdown { generation } => Some(*generation),
            Self::Handshake { .. } => None,
        }
    }
}

/// Encode one frame as a length-prefixed line.
///
/// The prefix is a fixed-width hex byte count followed by `\n`, so a reader can
/// size its buffer before reading any body.
///
/// # Errors
///
/// Returns the encoded length when it exceeds [`MAX_FRAME_BYTES`]. The frame is
/// not written, so an oversized payload can never be partially transmitted.
pub fn encode_frame<T: Serialize>(value: &T) -> Result<Vec<u8>, EncodeError> {
    let body =
        serde_json::to_vec(value).map_err(|error| EncodeError::Serialize(error.to_string()))?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(EncodeError::TooLarge {
            len: body.len(),
            max: MAX_FRAME_BYTES,
        });
    }
    let mut out = Vec::with_capacity(body.len() + 16);
    out.extend_from_slice(format!("{:08x}\n", body.len()).as_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Failure modes for [`encode_frame`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncodeError {
    /// The value could not be serialized.
    Serialize(String),
    /// The encoded frame exceeded [`MAX_FRAME_BYTES`].
    TooLarge {
        /// Encoded size that was rejected.
        len: usize,
        /// The cap that was exceeded.
        max: usize,
    },
}

impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Serialize(message) => write!(f, "could not encode frame: {message}"),
            Self::TooLarge { len, max } => {
                write!(f, "encoded frame of {len} bytes exceeds the {max}-byte cap")
            }
        }
    }
}

impl std::error::Error for EncodeError {}

/// Why a frame could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// The peer closed cleanly between frames.
    Closed,
    /// The peer closed mid-frame, or the transport failed.
    Io(String),
    /// The length prefix was not eight hex digits.
    MalformedLength(String),
    /// The announced length exceeded [`MAX_FRAME_BYTES`].
    TooLarge {
        /// Length that was announced.
        len: usize,
        /// The cap that was exceeded.
        max: usize,
    },
    /// The body was not valid UTF-8.
    NotUtf8,
    /// The body was not a valid frame.
    MalformedBody(String),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => f.write_str("connection closed"),
            Self::Io(message) => write!(f, "transport error: {message}"),
            Self::MalformedLength(message) => write!(f, "malformed length prefix: {message}"),
            Self::TooLarge { len, max } => {
                write!(
                    f,
                    "announced frame of {len} bytes exceeds the {max}-byte cap"
                )
            }
            Self::NotUtf8 => f.write_str("frame body was not valid UTF-8"),
            Self::MalformedBody(message) => write!(f, "malformed frame body: {message}"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// Extract exactly `len` bytes from `buf`, or report how many are still needed.
///
/// Returning `None` leaves the buffer untouched, so the caller can read more and
/// retry without losing a partial frame.
#[must_use]
pub fn take_exact(buf: &[u8], len: usize) -> Option<&[u8]> {
    if buf.len() < len {
        None
    } else {
        Some(&buf[..len])
    }
}

/// Parse a length prefix of exactly `LEN_PREFIX_BYTES` bytes.
pub fn parse_length(prefix: &[u8]) -> Result<usize, DecodeError> {
    if prefix.len() != LEN_PREFIX_BYTES {
        return Err(DecodeError::MalformedLength(format!(
            "expected {LEN_PREFIX_BYTES} bytes, got {}",
            prefix.len()
        )));
    }
    let text = std::str::from_utf8(prefix)
        .map_err(|error| DecodeError::MalformedLength(error.to_string()))?;
    let trimmed = text
        .strip_suffix('\n')
        .ok_or_else(|| DecodeError::MalformedLength("missing newline terminator".to_owned()))?;
    let len = usize::from_str_radix(trimmed, 16)
        .map_err(|error| DecodeError::MalformedLength(error.to_string()))?;
    if len > MAX_FRAME_BYTES {
        return Err(DecodeError::TooLarge {
            len,
            max: MAX_FRAME_BYTES,
        });
    }
    Ok(len)
}

/// Bytes in one length prefix, including its newline.
pub const LEN_PREFIX_BYTES: usize = 9;

/// Decide what a frontend should do with a frame it received.
#[must_use]
pub fn classify(frame: &FrontendFrame) -> FrameDisposition {
    match frame {
        FrontendFrame::VersionMismatch(_) => FrameDisposition::ExitIncompatible,
        FrontendFrame::ShuttingDown => FrameDisposition::ExitDaemonStopped,
        FrontendFrame::ProtocolError(_) => FrameDisposition::ExitProtocolError,
        FrontendFrame::Snapshot(_) | FrontendFrame::Hello(_) | FrontendFrame::ControlAck { .. } => {
            FrameDisposition::Continue
        }
    }
}

/// What a frontend must do after receiving a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameDisposition {
    /// Keep reading; the frame carried usable state or an acknowledgement.
    Continue,
    /// The daemon speaks a different protocol version. Exit with guidance
    /// rather than rendering frames whose meaning is unknown.
    ExitIncompatible,
    /// The daemon is stopping on purpose. This is not a transport failure.
    ExitDaemonStopped,
    /// The frame was a protocol error the frontend cannot continue past.
    ExitProtocolError,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clientd::snapshot::{FrontendSnapshot, SystemSnapshotDto};
    use crate::state::Reachability;

    fn hello() -> FrontendFrame {
        FrontendFrame::Hello(Box::new(HelloPayload {
            protocol_version: PROTOCOL_VERSION,
            version: "1.0.0".to_owned(),
            daemon_id: "0123456789abcdef".to_owned(),
            current_generation: 1,
        }))
    }

    #[test]
    fn a_frame_round_trips_through_the_length_prefix() {
        let encoded = encode_frame(&hello()).expect("encodes");
        let prefix = take_exact(&encoded, LEN_PREFIX_BYTES).expect("prefix present");
        let len = parse_length(prefix).expect("valid prefix");
        let body = take_exact(&encoded[LEN_PREFIX_BYTES..], len).expect("body present");
        let frame: FrontendFrame = serde_json::from_slice(body).expect("decodes");
        assert_eq!(frame, hello());
    }

    #[test]
    fn an_oversized_frame_is_rejected_before_transmission() {
        let huge = "x".repeat(MAX_FRAME_BYTES + 1);
        let error = encode_frame(&huge).expect_err("must refuse");
        match error {
            EncodeError::TooLarge { len, max } => {
                assert_eq!(max, MAX_FRAME_BYTES);
                assert!(len > MAX_FRAME_BYTES, "reported {len}");
            }
            other @ EncodeError::Serialize(_) => panic!("expected TooLarge, got {other:?}"),
        }
    }

    #[test]
    fn an_oversized_announced_length_is_refused_without_allocating() {
        let prefix = format!("{:08x}\n", MAX_FRAME_BYTES + 1);
        assert_eq!(
            parse_length(prefix.as_bytes()),
            Err(DecodeError::TooLarge {
                len: MAX_FRAME_BYTES + 1,
                max: MAX_FRAME_BYTES
            })
        );
    }

    #[test]
    fn a_malformed_length_prefix_is_reported() {
        assert!(matches!(
            parse_length(b"zzzzzzz\n"),
            Err(DecodeError::MalformedLength(_))
        ));
        // "12345678" is valid hex, so the failure case must be a genuinely
        // non-hex character, not a decimal-looking one.
        assert_eq!(parse_length(b"0000abcd\n"), Ok(0xabcd));
        assert!(matches!(
            parse_length(b"zzzzzzzz\n"),
            Err(DecodeError::MalformedLength(_))
        ));
        assert!(matches!(
            parse_length(b"1234"),
            Err(DecodeError::MalformedLength(_))
        ));
    }

    #[test]
    fn a_partial_frame_yields_none_and_consumes_nothing() {
        assert_eq!(take_exact(b"ab", 4), None);
        assert_eq!(take_exact(b"abcdef", 3), Some(&b"abc"[..]));
    }

    #[test]
    fn a_version_mismatch_is_an_exit_not_a_render() {
        assert_eq!(classify(&hello()), FrameDisposition::Continue);
        assert_eq!(
            classify(&FrontendFrame::VersionMismatch(VersionMismatchPayload {
                frontend: 1,
                daemon: 2,
            })),
            FrameDisposition::ExitIncompatible
        );
        assert_eq!(
            classify(&FrontendFrame::ShuttingDown),
            FrameDisposition::ExitDaemonStopped
        );
    }

    #[test]
    fn requests_carry_the_generation_they_will_be_acknowledged_with() {
        assert_eq!(
            DaemonRequest::ReloadConfig { generation: 7 }.generation(),
            Some(7)
        );
        assert_eq!(
            DaemonRequest::Shutdown { generation: 8 }.generation(),
            Some(8)
        );
        assert_eq!(
            DaemonRequest::Handshake {
                protocol_version: PROTOCOL_VERSION,
                version: "1.0.0".to_owned()
            }
            .generation(),
            None
        );
    }

    #[test]
    fn a_typical_snapshot_is_far_below_the_frame_cap() {
        let systems: Vec<SystemSnapshotDto> = (0..64)
            .map(|index| SystemSnapshotDto::placeholder(index, Reachability::Pending))
            .collect();
        let snapshot = FrontendSnapshot::empty(systems);
        let encoded = encode_frame(&FrontendFrame::Snapshot(Box::new(snapshot))).expect("encodes");
        assert!(
            encoded.len() < MAX_FRAME_BYTES / 4,
            "64 empty systems serialized to {} bytes",
            encoded.len()
        );
    }
}
