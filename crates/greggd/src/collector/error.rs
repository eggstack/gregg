//! Collector error taxonomy.
//!
//! Errors are structured so the daemon sampler can distinguish transient
//! states (warming, counter reset) from hard failures. The display format
//! intentionally omits raw `/proc` or `/sys` content; the diagnostic
//! information is the failing source category and a short message.

use thiserror::Error;

/// Category of a collection failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectErrorKind {
    /// The first counter baseline is not yet available. Percentages cannot be
    /// computed until a second sample exists.
    Warming,
    /// A native source (file, kernel interface) was unavailable or denied.
    SourceUnavailable,
    /// A native source returned content that did not match the expected
    /// schema. Carries the source category but not raw content.
    Parse,
    /// The previous counter sample is no longer comparable to the new one,
    /// typically because counters were reset or wrapped. The collector has
    /// discarded the baseline; the next call will return `Warming`.
    CounterReset,
    /// Normalized numeric output is not representable as a finite percentage.
    /// Indicates a deeper arithmetic bug rather than a transient condition.
    Numeric,
    /// Identity could not be fully determined. The collector may still be
    /// able to produce samples; the daemon should report a degraded identity
    /// in the snapshot and continue.
    IdentityFallback,
}

/// A single collector error.
///
/// `kind` is the machine-readable category; `message` is a short, safe
/// human-readable explanation that never includes raw file contents,
/// filesystem paths, or platform-private structures. The `source` field
/// carries an optional chained error for tracing/debug logs only.
#[derive(Debug, Error)]
#[error("{kind}: {message}")]
pub struct CollectError {
    pub kind: CollectErrorKind,
    pub message: String,
    #[source]
    pub source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl CollectError {
    /// Construct a new error with the given kind and message.
    #[must_use]
    pub fn new(kind: CollectErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            source: None,
        }
    }

    /// Attach a chained source error.
    #[must_use]
    pub fn with_source<E>(mut self, source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        self.source = Some(Box::new(source));
        self
    }

    /// Convenience for a `Warming` error.
    #[must_use]
    pub fn warming(message: impl Into<String>) -> Self {
        Self::new(CollectErrorKind::Warming, message)
    }

    /// Convenience for a counter-reset error.
    #[must_use]
    pub fn counter_reset(message: impl Into<String>) -> Self {
        Self::new(CollectErrorKind::CounterReset, message)
    }
}

impl std::fmt::Display for CollectErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::Warming => "warming",
            Self::SourceUnavailable => "source unavailable",
            Self::Parse => "parse failure",
            Self::CounterReset => "counter reset",
            Self::Numeric => "numeric failure",
            Self::IdentityFallback => "identity fallback",
        };
        f.write_str(label)
    }
}
