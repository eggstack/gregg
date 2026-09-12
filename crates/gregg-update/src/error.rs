//! Shared update error taxonomy.
//!
//! One authoritative error classification for `gregg update` and
//! `greggd update` (Plan 104). The `RestartFailed` variant is produced only
//! by daemon lifecycle coordination in `greggd`; the shared mechanism never
//! constructs it and knows nothing about service managers — the payload is
//! an opaque message string.

/// Errors that can occur during a binary-first self-update.
#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    /// Could not determine the current executable path.
    #[error("failed to determine current executable: {0}")]
    CurrentExe(String),
    /// `curl` is not available in `PATH`.
    #[error("curl is not available: {0}. Install curl or update manually from https://github.com/eggstack/gregg/releases")]
    CurlMissing(String),
    /// `cargo` is not available in `PATH`.
    #[error("cargo is not available: {0}. Install Rust from https://rustup.rs or download the release asset manually")]
    CargoMissing(String),
    /// Version lookup against crates.io failed.
    #[error("version lookup failed: {0}")]
    VersionLookup(String),
    /// A version string is not a valid stable `MAJOR.MINOR.PATCH`.
    #[error("invalid version '{input}': {reason}")]
    InvalidVersion {
        /// The offending version input.
        input: String,
        /// Why it was rejected.
        reason: String,
    },
    /// The release asset download failed for a non-404 reason.
    #[error("release download failed for {url}: {reason}")]
    ReleaseDownloadFailed {
        /// Asset URL that failed.
        url: String,
        /// Transport failure detail.
        reason: String,
    },
    /// The checksum file could not be retrieved or parsed.
    #[error("checksum retrieval failed: {0}")]
    ChecksumRetrieval(String),
    /// The downloaded bytes do not match the published checksum.
    #[error("checksum mismatch for {file}: expected {expected}, actual {actual}")]
    ChecksumMismatch {
        /// Downloaded file path.
        file: String,
        /// Expected hex digest.
        expected: String,
        /// Actual hex digest.
        actual: String,
    },
    /// The staged candidate failed identity/version verification.
    #[error("candidate identity/version mismatch: {0}")]
    CandidateMismatch(String),
    /// The install location is not writable by this invocation.
    #[error("permission denied: {message}. Rerun: {elevated}")]
    PermissionDenied {
        /// What was denied.
        message: String,
        /// Exact elevated command to rerun.
        elevated: String,
    },
    /// The Cargo fallback build/install failed.
    #[error("cargo fallback failed: {0}")]
    CargoFallback(String),
    /// Executable replacement failed.
    #[error("replacement failed: {0}")]
    Replacement(String),
    /// Daemon restart after replacement failed (constructed only by `greggd`
    /// lifecycle coordination; never by the shared mechanism).
    #[error("restart failed: {0}")]
    RestartFailed(String),
    /// An I/O error outside the categories above.
    #[error("I/O error: {0}")]
    Io(String),
}
