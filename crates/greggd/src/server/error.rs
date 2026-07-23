//! Error types for the HTTP server module.

use std::fmt;

/// Errors returned by the HTTP server bind/listen path.
#[derive(Debug)]
pub enum ServerError {
    /// The configured address could not be bound.
    Bind(std::io::Error),
    /// The server failed while running.
    Runtime(std::io::Error),
}

impl fmt::Display for ServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bind(e) => write!(f, "failed to bind: {e}"),
            Self::Runtime(e) => write!(f, "server runtime error: {e}"),
        }
    }
}

impl std::error::Error for ServerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Bind(e) | Self::Runtime(e) => Some(e),
        }
    }
}

/// Configuration validation errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerConfigError {
    /// Port is outside the valid range 1..=65535.
    InvalidPort(u16),
    /// Sample interval is outside the valid range 250..=60000 ms.
    InvalidSampleInterval(u64),
}

impl fmt::Display for ServerConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPort(p) => write!(f, "port {p} is outside valid range 1..=65535"),
            Self::InvalidSampleInterval(ms) => {
                write!(
                    f,
                    "sample_interval_ms {ms} is outside valid range 250..=60000"
                )
            }
        }
    }
}

impl std::error::Error for ServerConfigError {}
