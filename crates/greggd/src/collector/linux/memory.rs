//! Linux memory and swap normalization from `/proc/meminfo`.
//!
//! - `MemTotal` is the canonical total. Used = total - available.
//! - `MemAvailable` is preferred for availability when present; if missing
//!   (older kernels), a conservative fallback sums `MemFree + Buffers +
//!   Cached + SReclaimable`. Avoids double-counting by using the same
//!   categorization the kernel uses for `MemAvailable` minus inactive file
//!   pages, but the version-1 collector does not need to mirror the kernel
//!   exactly; we only need a safe, monotonic lower bound.
//! - `SwapTotal - SwapFree` yields `swap_used`. Zero swap produces a valid
//!   zero reading with no division by zero.
//!
//! All kilobyte values are converted to bytes with checked multiplication
//! so a malicious or corrupt source cannot silently overflow `u64`.

use gregg_protocol::{MemoryMetrics, SwapMetrics};

use crate::collector::error::{CollectError, CollectErrorKind};
use crate::collector::linux::source::ParsedMeminfo;

/// Parsed memory information normalized into the collector's wire shape.
#[derive(Debug, Clone, PartialEq)]
pub struct MemorySample {
    pub used_bytes: u64,
    pub total_bytes: u64,
    pub fallback_used: bool,
}

impl MemorySample {
    /// Convert into the wire [`MemoryMetrics`], clamping `usage_pct` to the
    /// closed `0.0..=100.0` interval.
    #[must_use]
    pub fn into_metrics(self) -> MemoryMetrics {
        let usage_pct = percent(self.used_bytes, self.total_bytes);
        MemoryMetrics {
            used_bytes: self.used_bytes,
            total_bytes: self.total_bytes,
            usage_pct,
        }
    }
}

/// Parsed swap information normalized into the collector's wire shape.
#[derive(Debug, Clone, PartialEq)]
pub struct SwapSample {
    pub used_bytes: u64,
    pub total_bytes: u64,
}

impl SwapSample {
    /// Convert into the wire [`SwapMetrics`], handling zero total swap by
    /// returning `usage_pct == 0.0` rather than dividing by zero.
    #[must_use]
    pub fn into_metrics(self) -> SwapMetrics {
        let usage_pct = percent(self.used_bytes, self.total_bytes);
        SwapMetrics {
            used_bytes: self.used_bytes,
            total_bytes: self.total_bytes,
            usage_pct,
        }
    }
}

/// Parse `/proc/meminfo` content into a [`ParsedMeminfo`].
///
/// The parser tolerates the standard `FieldName:` and
/// `FieldName:NNN kB` formats used by Linux. Unknown keys are ignored.
pub fn parse_meminfo(raw: &str) -> Result<ParsedMeminfo, CollectError> {
    let mut info = ParsedMeminfo::default();
    for line in raw.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let value_token = rest.split_whitespace().next();
        let value_kb: Option<u64> = match value_token {
            Some(token) => match token.parse::<u64>() {
                Ok(n) => Some(n),
                Err(_) => continue,
            },
            None => continue,
        };
        match key.trim() {
            "MemTotal" => info.mem_total_kb = value_kb,
            "MemAvailable" => info.mem_available_kb = value_kb,
            "MemFree" => info.mem_free_kb = value_kb,
            "Buffers" => info.buffers_kb = value_kb,
            "Cached" => info.cached_kb = value_kb,
            "SReclaimable" => info.s_reclaimable_kb = value_kb,
            "SwapTotal" => info.swap_total_kb = value_kb,
            "SwapFree" => info.swap_free_kb = value_kb,
            _ => {}
        }
    }
    Ok(info)
}

/// Compute memory metrics, choosing `MemAvailable` when present or the
/// documented fallback otherwise.
pub fn compute_memory(info: &ParsedMeminfo) -> Result<MemorySample, CollectError> {
    let total_kb = info.mem_total_kb.ok_or_else(|| {
        CollectError::new(CollectErrorKind::Parse, "/proc/meminfo missing MemTotal")
    })?;
    let total_bytes = kb_to_bytes(total_kb)?;

    let (available_bytes, fallback_used) = if let Some(avail_kb) = info.mem_available_kb {
        (kb_to_bytes(avail_kb)?, false)
    } else {
        // Fallback: MemFree + Buffers + Cached + SReclaimable. Missing
        // subfields are treated as zero. Document this in test fixtures.
        let free = info.mem_free_kb.unwrap_or(0);
        let buffers = info.buffers_kb.unwrap_or(0);
        let cached = info.cached_kb.unwrap_or(0);
        let reclaimable = info.s_reclaimable_kb.unwrap_or(0);
        let avail_kb = free
            .checked_add(buffers)
            .and_then(|v| v.checked_add(cached))
            .and_then(|v| v.checked_add(reclaimable))
            .ok_or_else(|| {
                CollectError::new(
                    CollectErrorKind::Numeric,
                    "memory fallback addition overflowed u64",
                )
            })?;
        (kb_to_bytes(avail_kb)?, true)
    };

    if available_bytes > total_bytes {
        // Kernel counter races can transiently exceed the total. Treat as a
        // normalization error so callers can decide whether to clamp or
        // surface the issue.
        return Err(CollectError::new(
            CollectErrorKind::Numeric,
            "available memory exceeds total memory",
        ));
    }

    let used_bytes = total_bytes - available_bytes;
    Ok(MemorySample {
        used_bytes,
        total_bytes,
        fallback_used,
    })
}

/// Compute swap metrics. Zero swap is valid and yields zero usage.
pub fn compute_swap(info: &ParsedMeminfo) -> Result<SwapSample, CollectError> {
    let total_kb = info.swap_total_kb.unwrap_or(0);
    let free_kb = info.swap_free_kb.unwrap_or(0);

    let total_bytes = kb_to_bytes(total_kb)?;
    // `SwapFree` may exceed `SwapTotal` on certain kernels with zram-style
    // compressed swap. Clamp defensively and surface the clamp as zero used
    // rather than overflowing.
    let free_clamped = free_kb.min(total_kb);
    let used_kb = total_kb - free_clamped;
    let used_bytes = kb_to_bytes(used_kb)?;

    Ok(SwapSample {
        used_bytes,
        total_bytes,
    })
}

fn kb_to_bytes(kb: u64) -> Result<u64, CollectError> {
    kb.checked_mul(1024).ok_or_else(|| {
        CollectError::new(
            CollectErrorKind::Numeric,
            "kilobyte-to-byte conversion overflowed u64",
        )
    })
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "byte counts > 2^52 saturate to 100% anyway; protocol saturation is documented"
)]
fn percent(used: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        let pct = (used as f64) * 100.0 / (total as f64);
        (pct as f32).clamp(0.0, 100.0)
    }
}
