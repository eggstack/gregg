//! Native collection error taxonomy.
//!
//! Mirrors `greggd::collector::error` so the `greggd` compatibility boundary
//! can preserve exact `CollectErrorKind` meaning without lossy mapping.

use thiserror::Error;

/// Category of a collection failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectErrorKind {
    /// The first counter baseline is not yet available.
    Warming,
    /// A native source was unavailable or denied.
    SourceUnavailable,
    /// A native source returned unparseable content.
    Parse,
    /// The previous counter sample is no longer comparable (reset/wrap/zero
    /// delta). The collector has discarded the baseline.
    CounterReset,
    /// Normalized numeric output is not representable as a finite percentage.
    Numeric,
    /// Identity could not be fully determined. Reserved; sampling fails
    /// without publishing a fabricated identity.
    IdentityFallback,
}

/// A single collection error.
#[derive(Debug, Error)]
#[error("{kind}: {message}")]
pub struct CollectError {
    /// Machine-readable category.
    pub kind: CollectErrorKind,
    /// Short human-readable explanation (never raw file contents).
    pub message: String,
    /// Optional chained source for tracing/debug logs only.
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
