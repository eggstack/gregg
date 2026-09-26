//! `gregg-host`: native host telemetry acquisition.
//!
//! Protocol-neutral, synchronous, runtime-neutral collection for Linux,
//! macOS, Windows, and FreeBSD with explicit warmup/reset semantics. See
//! `README.md` for platform support and `model.rs` for the neutral types.
//!
//! # Design rules
//!
//! - Never spawns external commands. Linux uses procfs/sysfs; macOS uses
//!   Mach/sysctl/libc/IOKit; Windows uses Win32; FreeBSD uses
//!   sysctl/libdevstat/ifmib.
//! - Never owns a clock. `Instant::now()` stays at the existing disk/network
//!   collection boundary; the caller stamps wall-clock time.
//! - All percentage normalization, counter-delta handling, and warming state
//!   live behind the collector trait, not in the protocol crate.
//! - Errors are typed so callers can distinguish warming baselines from hard
//!   failures.
//! - Unsafe is confined to documented platform FFI/source modules.

pub mod drives;
pub mod error;
pub mod model;
pub mod rate;
pub mod slow_probe;

#[cfg(target_os = "freebsd")]
pub mod freebsd;
#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod windows;

pub use drives::{normalize as normalize_drives, DriveCandidate};
pub use error::{CollectError, CollectErrorKind};
pub use model::{
    CollectionLimits, CommitMetrics, DiskIoMetrics, DiskIoPayload, DriveMetrics, HostCapabilities,
    HostIdentity, HostSample, LoadAverage, MemoryMetrics, NetworkInterfaceMetrics, NetworkPayload,
    SwapMetrics,
};
pub use rate::{CounterBaselines, CounterRates, CounterSample};
pub use slow_probe::DriveRefreshCache;

use error::{CollectError as CollectErrorInner, CollectErrorKind as CollectErrorKindInner};

/// Shared clamped percentage normalization for byte ratios.
///
/// Zero total yields `0.0` rather than a division by zero; the result is
/// clamped to the closed `0.0..=100.0` interval.
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
pub fn clamped_usage_pct(used_bytes: u64, total_bytes: u64) -> f32 {
    if total_bytes == 0 {
        0.0
    } else if used_bytes >= total_bytes {
        100.0
    } else {
        let pct = (used_bytes as f64 / total_bytes as f64) * 100.0;
        finalize_percentage(pct).unwrap_or(0.0)
    }
}

/// Validate, narrow, and clamp a computed percentage once at the collector
/// boundary so platform implementations do not drift in their arithmetic.
pub fn finalize_percentage(value: f64) -> Result<f32, CollectErrorInner> {
    if !value.is_finite() {
        return Err(CollectErrorInner::new(
            CollectErrorKindInner::Numeric,
            "percentage is not finite",
        ));
    }
    #[allow(clippy::cast_possible_truncation)]
    let as_f32 = value.clamp(0.0, 100.0) as f32;
    if !as_f32.is_finite() || !(0.0..=100.0).contains(&as_f32) {
        return Err(CollectErrorInner::new(
            CollectErrorKindInner::Numeric,
            "percentage outside closed 0..=100 interval after conversion",
        ));
    }
    Ok(as_f32)
}

/// Shared collector contract implemented by every platform collector.
///
/// The contract is intentionally minimal: it owns identity collection and one
/// incremental sample. The caller owns cadence and clock.
pub trait HostCollector: Send {
    /// Read identity fields once and cache them inside the collector.
    fn identity(&self) -> Result<HostIdentity, CollectError>;

    /// Take one native sample.
    ///
    /// The first call after construction is expected to return
    /// [`CollectErrorKind::Warming`] because percentage metrics require a
    /// second reading. Once two valid samples exist the collector returns a
    /// normalized [`HostSample`].
    fn sample(&mut self) -> Result<HostSample, CollectError>;

    /// Per-platform capability flags.
    fn capabilities(&self) -> HostCapabilities;
}
