//! Test-only fixture builders for [`StatusSnapshot`].
//!
//! Enabled by the `test_support` feature. Production builds do not compile
//! this module, so its dependencies stay out of the published crate.

use crate::{
    snapshot::{
        CpuMetrics, LoadAverage, MemoryMetrics, MetricCapabilities, StatusSnapshot, SwapMetrics,
        SystemIdentity,
    },
    SCHEMA_VERSION_V1,
};

/// Identity fields shared by every test fixture.
#[derive(Debug, Clone)]
pub struct IdentityFixture {
    pub name: &'static str,
    pub hostname: &'static str,
    pub os_name: &'static str,
    pub os_version: &'static str,
    pub kernel_name: &'static str,
    pub kernel_release: &'static str,
    pub architecture: &'static str,
}

impl IdentityFixture {
    /// Linux desktop identity used by default Linux fixtures.
    pub const fn linux() -> Self {
        Self {
            name: "deadpool",
            hostname: "deadpool.local",
            os_name: "linux",
            os_version: "Ubuntu 24.04",
            kernel_name: "Linux",
            kernel_release: "6.8.0-31-generic",
            architecture: "x86_64",
        }
    }

    /// macOS Apple Silicon identity used by default macOS fixtures.
    pub const fn macos() -> Self {
        Self {
            name: "mac-mini",
            hostname: "mac-mini.local",
            os_name: "macos",
            os_version: "15.0",
            kernel_name: "Darwin",
            kernel_release: "24.0.0",
            architecture: "arm64",
        }
    }

    fn into_identity(self) -> SystemIdentity {
        SystemIdentity {
            name: self.name.into(),
            hostname: self.hostname.into(),
            os_name: self.os_name.into(),
            os_version: self.os_version.into(),
            kernel_name: self.kernel_name.into(),
            kernel_release: self.kernel_release.into(),
            architecture: self.architecture.into(),
        }
    }
}

/// Builder for a Linux snapshot. Defaults assume a healthy desktop host.
#[derive(Debug, Clone)]
pub struct LinuxSnapshotBuilder {
    identity: IdentityFixture,
    logical_cores: u32,
    usage_pct: f32,
    iowait_pct: f32,
    load: LoadAverage,
    used_bytes: u64,
    total_bytes: u64,
    swap_used_bytes: u64,
    swap_total_bytes: u64,
    sample_interval_ms: u64,
    observed_at_unix_ms: u64,
}

impl Default for LinuxSnapshotBuilder {
    fn default() -> Self {
        Self {
            identity: IdentityFixture::linux(),
            logical_cores: 8,
            usage_pct: 25.2,
            iowait_pct: 0.4,
            load: LoadAverage {
                one: 1.32,
                five: 0.91,
                fifteen: 0.62,
            },
            used_bytes: 5_900_000_000,
            total_bytes: 15_600_000_000,
            swap_used_bytes: 0,
            swap_total_bytes: 4_000_000_000,
            sample_interval_ms: 1000,
            observed_at_unix_ms: 1_716_460_800_000,
        }
    }
}

impl LinuxSnapshotBuilder {
    /// Override logical core count.
    #[must_use]
    pub const fn logical_cores(mut self, cores: u32) -> Self {
        self.logical_cores = cores;
        self
    }

    /// Override total CPU busy percentage.
    #[must_use]
    pub const fn usage_pct(mut self, pct: f32) -> Self {
        self.usage_pct = pct;
        self
    }

    /// Override aggregate CPU I/O-wait percentage.
    #[must_use]
    pub const fn iowait_pct(mut self, pct: f32) -> Self {
        self.iowait_pct = pct;
        self
    }

    /// Override the load-average triple.
    #[must_use]
    pub const fn load(mut self, one: f32, five: f32, fifteen: f32) -> Self {
        self.load = LoadAverage { one, five, fifteen };
        self
    }

    /// Override used and total memory bytes.
    #[must_use]
    pub const fn memory(mut self, used_bytes: u64, total_bytes: u64) -> Self {
        self.used_bytes = used_bytes;
        self.total_bytes = total_bytes;
        self
    }

    /// Override used and total swap bytes.
    #[must_use]
    pub const fn swap(mut self, used_bytes: u64, total_bytes: u64) -> Self {
        self.swap_used_bytes = used_bytes;
        self.swap_total_bytes = total_bytes;
        self
    }

    /// Override the sampling interval.
    #[must_use]
    pub const fn sample_interval_ms(mut self, ms: u64) -> Self {
        self.sample_interval_ms = ms;
        self
    }

    /// Override the observation timestamp.
    #[must_use]
    pub const fn observed_at_unix_ms(mut self, ms: u64) -> Self {
        self.observed_at_unix_ms = ms;
        self
    }

    /// Build the snapshot and run [`StatusSnapshot::validate`].
    ///
    /// # Panics
    ///
    /// Panics if the resulting snapshot violates a protocol invariant. Tests
    /// build snapshots from well-known values, so a panic here indicates a
    /// regression in the builder defaults.
    #[must_use]
    pub fn build(self) -> StatusSnapshot {
        let snap = StatusSnapshot {
            schema_version: SCHEMA_VERSION_V1,
            observed_at_unix_ms: self.observed_at_unix_ms,
            sample_interval_ms: self.sample_interval_ms,
            capabilities: MetricCapabilities { cpu_iowait: true },
            system: self.identity.into_identity(),
            cpu: CpuMetrics {
                logical_cores: self.logical_cores,
                usage_pct: self.usage_pct,
                iowait_pct: Some(self.iowait_pct),
            },
            load: self.load,
            memory: MemoryMetrics {
                used_bytes: self.used_bytes,
                total_bytes: self.total_bytes,
                usage_pct: percent(self.used_bytes, self.total_bytes),
            },
            swap: SwapMetrics {
                used_bytes: self.swap_used_bytes,
                total_bytes: self.swap_total_bytes,
                usage_pct: percent(self.swap_used_bytes, self.swap_total_bytes),
            },
        };
        snap.validate().expect("linux snapshot validates");
        snap
    }
}

/// Builder for a macOS snapshot. macOS has no I/O-wait, so `cpu_iowait` is
/// always `false` and `iowait_pct` is always `None`.
#[derive(Debug, Clone)]
pub struct MacosSnapshotBuilder {
    identity: IdentityFixture,
    logical_cores: u32,
    usage_pct: f32,
    load: LoadAverage,
    used_bytes: u64,
    total_bytes: u64,
    swap_used_bytes: u64,
    swap_total_bytes: u64,
    sample_interval_ms: u64,
    observed_at_unix_ms: u64,
}

impl Default for MacosSnapshotBuilder {
    fn default() -> Self {
        Self {
            identity: IdentityFixture::macos(),
            logical_cores: 8,
            usage_pct: 18.7,
            load: LoadAverage {
                one: 2.10,
                five: 1.85,
                fifteen: 1.40,
            },
            used_bytes: 9_000_000_000,
            total_bytes: 16_000_000_000,
            swap_used_bytes: 0,
            swap_total_bytes: 0,
            sample_interval_ms: 1000,
            observed_at_unix_ms: 1_716_460_800_000,
        }
    }
}

impl MacosSnapshotBuilder {
    /// Override logical core count.
    #[must_use]
    pub const fn logical_cores(mut self, cores: u32) -> Self {
        self.logical_cores = cores;
        self
    }

    /// Override total CPU busy percentage.
    #[must_use]
    pub const fn usage_pct(mut self, pct: f32) -> Self {
        self.usage_pct = pct;
        self
    }

    /// Override the load-average triple.
    #[must_use]
    pub const fn load(mut self, one: f32, five: f32, fifteen: f32) -> Self {
        self.load = LoadAverage { one, five, fifteen };
        self
    }

    /// Override used and total memory bytes.
    #[must_use]
    pub const fn memory(mut self, used_bytes: u64, total_bytes: u64) -> Self {
        self.used_bytes = used_bytes;
        self.total_bytes = total_bytes;
        self
    }

    /// Override used and total swap bytes.
    #[must_use]
    pub const fn swap(mut self, used_bytes: u64, total_bytes: u64) -> Self {
        self.swap_used_bytes = used_bytes;
        self.swap_total_bytes = total_bytes;
        self
    }

    /// Override the sampling interval.
    #[must_use]
    pub const fn sample_interval_ms(mut self, ms: u64) -> Self {
        self.sample_interval_ms = ms;
        self
    }

    /// Override the observation timestamp.
    #[must_use]
    pub const fn observed_at_unix_ms(mut self, ms: u64) -> Self {
        self.observed_at_unix_ms = ms;
        self
    }

    /// Build the snapshot and run [`StatusSnapshot::validate`].
    ///
    /// # Panics
    ///
    /// Panics if the resulting snapshot violates a protocol invariant. Tests
    /// build snapshots from well-known values, so a panic here indicates a
    /// regression in the builder defaults.
    #[must_use]
    pub fn build(self) -> StatusSnapshot {
        let snap = StatusSnapshot {
            schema_version: SCHEMA_VERSION_V1,
            observed_at_unix_ms: self.observed_at_unix_ms,
            sample_interval_ms: self.sample_interval_ms,
            capabilities: MetricCapabilities { cpu_iowait: false },
            system: self.identity.into_identity(),
            cpu: CpuMetrics {
                logical_cores: self.logical_cores,
                usage_pct: self.usage_pct,
                iowait_pct: None,
            },
            load: self.load,
            memory: MemoryMetrics {
                used_bytes: self.used_bytes,
                total_bytes: self.total_bytes,
                usage_pct: percent(self.used_bytes, self.total_bytes),
            },
            swap: SwapMetrics {
                used_bytes: self.swap_used_bytes,
                total_bytes: self.swap_total_bytes,
                usage_pct: percent(self.swap_used_bytes, self.swap_total_bytes),
            },
        };
        snap.validate().expect("macos snapshot validates");
        snap
    }
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "byte counts > 2^52 are clamped and the percentage saturates anyway"
)]
fn percent(used: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        let pct = (used as f64) * 100.0 / (total as f64);
        (pct as f32).clamp(0.0, 100.0)
    }
}
