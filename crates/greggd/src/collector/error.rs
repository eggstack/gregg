//! Collector error taxonomy (Plan 135 compatibility facade).
//!
//! Production errors come from [`gregg_host`]. This module re-exports the
//! host taxonomy so `greggd::collector::error::{CollectError,
//! CollectErrorKind}` paths and `CollectErrorKind` meaning are preserved
//! exactly at the compatibility boundary.
//!
//! Errors are structured so the daemon sampler can distinguish transient
//! states (warming, counter reset) from hard failures. The display format
//! intentionally omits raw `/proc` or `/sys` content; the diagnostic
//! information is the failing source category and a short message.

pub use gregg_host::error::{CollectError, CollectErrorKind};
