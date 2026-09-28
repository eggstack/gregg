//! In-process source abstraction for procfs reads.
//!
//! Production code reads from `/proc/stat`, `/proc/loadavg`, `/proc/meminfo`,
//! `/proc/sys/kernel/{osrelease,hostname}`, `/sys/devices/system/cpu`, and
//! `/etc/os-release` through the [`ProcSource::production`] constructor.
//! Tests construct a [`ProcSource`] with explicit file contents so they can
//! exercise edge cases without depending on the host `/proc` filesystem.
//!
//! No external commands are invoked for metrics collection.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::{CollectError, CollectErrorKind};

/// Plan 146: maximum number of distinct CPU identities Gregg will materialize
/// from a single CPU-list source file.
///
/// This is a *cardinality* bound, not a maximum numeric CPU ID. Linux CPU
/// numbers are identities and need not be dense `0..N-1`, so a sparse list
/// such as `0,2,10000` is valid and must keep its exact identities. Reusing
/// the collector's existing logical-core safety target keeps one number for
/// "how much CPU topology Gregg accepts".
const MAX_CPU_SET_MEMBERS: usize = crate::linux::MAX_LOGICAL_CORES;

/// Lowest-level read trait used by the Linux collector.
///
/// `read_to_string` returns the file contents or a structured
/// [`CollectError`] distinguishing "missing" from "permission denied" via the
/// kind. `available_parallelism` returns the kernel-reported logical core
/// count, or `None` if the platform refuses to provide one.
pub trait FileSource: Send + Sync + std::fmt::Debug {
    /// Read the entire contents of the named file.
    fn read_to_string(&self, path: &Path) -> Result<String, CollectError>;

    /// Enumerate immediate children of a native directory.
    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>, CollectError>;

    /// Test whether a native path exists, including a sysfs symlink.
    fn path_exists(&self, path: &Path) -> bool;

    /// Return the kernel-reported logical core count, if known.
    fn available_parallelism(&self) -> Option<usize>;

    /// Read native filesystem capacity for a mounted path.
    fn statvfs(&self, path: &Path) -> Result<RawStatvfs, CollectError>;

    /// Downcast helper used by tests to mutate fixture content after the
    /// source has been wrapped in an `Arc`. Production implementations return
    /// `None`.
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any>
    where
        Self: 'static,
    {
        None
    }
}

/// Plan 145: parsed CPU identity set, bounded by [`MAX_CPU_SET_MEMBERS`].
///
/// Implements the cpulist grammar accepted by `affected_cpus`,
/// `related_cpus`, and `online`: single IDs, comma- or whitespace-separated
/// values, and inclusive ranges. Identities are preserved exactly, so a
/// sparse Linux CPU numbering is represented without remapping or clamping.
/// The set keeps its members sorted so intersections are deterministic.
///
/// Plan 146: this type is private to the Linux source implementation. It is
/// an internal representation detail, not part of Gregg's reusable API.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct CpuIdSet {
    ids: BTreeSet<u32>,
}

impl CpuIdSet {
    /// Number of identities in the set.
    #[must_use]
    fn len(&self) -> usize {
        self.ids.len()
    }

    /// Whether `id` is a member of the set.
    #[must_use]
    fn contains(&self, id: u32) -> bool {
        self.ids.contains(&id)
    }

    /// Iterate identities in ascending order.
    fn iter(&self) -> impl Iterator<Item = u32> + '_ {
        self.ids.iter().copied()
    }
}

/// Plan 145: cached Linux `CPUFreq` policy structure.
///
/// Membership is structural (the `related_cpus` identity set) while
/// `cpuinfo_cur_freq` / `scaling_cur_freq` are current values read every
/// sample. The cache stores `related_cpus` once per policy while the
/// policy directory set is unchanged; the live `online` set is read once
/// per sample and intersected with the cached `related_cpus` to derive
/// the dynamic policy weight, so same-cardinality online membership
/// changes are visible without re-reading structural membership files.
#[derive(Debug, Clone, Default)]
pub struct CpuFreqStructuralCache {
    /// Policy directory paths in deterministic order; refreshed every sample
    /// so add/remove is observable immediately.
    policies: Vec<PathBuf>,
    /// Structural `related_cpus` identity sets per policy. Refreshed only
    /// when the policy set or a per-policy structural read fails.
    related: std::collections::HashMap<PathBuf, CpuIdSet>,
    /// Last successful global online-CPU identity read. Retained for
    /// in-module tests and internal diagnostics; there is no production
    /// reader.
    online: Option<CpuIdSet>,
}

impl CpuFreqStructuralCache {
    /// Plan 146: static intersection count between two identity sets, using
    /// the smaller set as the iteration driver so the result is
    /// `O(min(|a|, |b|))`. Private because the live weighting decision is an
    /// internal implementation detail.
    fn intersection_count(a: &CpuIdSet, b: &CpuIdSet) -> usize {
        if a.len() <= b.len() {
            a.iter().filter(|id| b.contains(*id)).count()
        } else {
            b.iter().filter(|id| a.contains(*id)).count()
        }
    }
}

/// Procfs-flavoured [`FileSource`].
///
/// Holds an inner [`FileSource`] that performs actual I/O and caches the
/// contents of `/etc/os-release` and the logical core count so identity reads
/// are cheap on the hot path. Tests inject a [`MemorySource`] to feed fixture
/// content.
#[derive(Clone, Debug)]
pub struct ProcSource {
    inner: Arc<dyn FileSource>,
    os_release_override: Option<PathBuf>,
    logical_cores: Option<usize>,
    stat_path: PathBuf,
    loadavg_path: PathBuf,
    meminfo_path: PathBuf,
}

impl ProcSource {
    /// Construct a procfs source pointing at the live host filesystem.
    #[must_use]
    pub fn production() -> Self {
        Self {
            inner: Arc::new(HostSource),
            os_release_override: None,
            logical_cores: None,
            stat_path: PathBuf::from("/proc/stat"),
            loadavg_path: PathBuf::from("/proc/loadavg"),
            meminfo_path: PathBuf::from("/proc/meminfo"),
        }
    }

    /// Read-only access to the inner [`FileSource`].
    #[must_use]
    pub fn inner(&self) -> &Arc<dyn FileSource> {
        &self.inner
    }

    /// Construct a procfs source backed by an arbitrary [`FileSource`].
    ///
    /// Tests typically pass a [`MemorySource`] seeded with fixture contents.
    #[must_use]
    pub fn for_source(inner: Arc<dyn FileSource>) -> Self {
        Self {
            inner,
            os_release_override: None,
            logical_cores: None,
            stat_path: PathBuf::from("/proc/stat"),
            loadavg_path: PathBuf::from("/proc/loadavg"),
            meminfo_path: PathBuf::from("/proc/meminfo"),
        }
    }

    /// Convenience: build a procfs source directly from a [`MemorySource`].
    /// Avoids the `Arc` dance for tests.
    #[must_use]
    pub fn for_memory(inner: MemorySource) -> Self {
        Self::for_source(Arc::new(inner))
    }

    /// Borrow the underlying [`MemorySource`] when one was supplied. Used
    /// by tests to populate additional files after construction.
    #[must_use]
    pub fn memory_source_mut(&mut self) -> Option<&mut MemorySource> {
        let arc = Arc::get_mut(&mut self.inner)?;
        arc.as_any_mut()?.downcast_mut::<MemorySource>()
    }

    /// Override the `/etc/os-release` path. Production uses the well-known
    /// absolute path; tests usually substitute a fixture file.
    #[must_use]
    pub fn with_os_release_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.os_release_override = Some(path.into());
        self
    }

    /// Override the logical core count cached by the source. When `None` the
    /// collector falls back to [`FileSource::available_parallelism`].
    #[must_use]
    pub fn with_logical_cores(mut self, cores: usize) -> Self {
        self.logical_cores = Some(cores);
        self
    }

    /// Override the `/proc/stat` path. Tests use this to inject malformed
    /// or unusual content.
    #[must_use]
    pub fn with_stat_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.stat_path = path.into();
        self
    }

    /// Plan 141: switch the stat path on a live collector without
    /// requiring unique ownership of the inner fixture source.
    #[cfg(test)]
    pub fn set_stat_path(&mut self, path: PathBuf) {
        self.stat_path = path;
    }

    /// Override the `/proc/loadavg` path.
    #[must_use]
    pub fn with_loadavg_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.loadavg_path = path.into();
        self
    }

    /// Override the `/proc/meminfo` path.
    #[must_use]
    pub fn with_meminfo_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.meminfo_path = path.into();
        self
    }

    /// Read the contents of `/proc/stat` for CPU sampling.
    pub fn read_proc_stat(&self) -> Result<ParsedProcStat, CollectError> {
        let raw = self.read_path(&self.stat_path)?;
        cpu::parse_proc_stat(&raw)
    }

    /// Read the contents of `/proc/loadavg` and return the raw string.
    pub fn read_proc_loadavg(&self) -> Result<String, CollectError> {
        self.read_path(&self.loadavg_path)
    }

    /// Read the contents of `/proc/meminfo` for memory and swap sampling.
    pub fn read_proc_meminfo(&self) -> Result<ParsedMeminfo, CollectError> {
        let raw = self.read_path(&self.meminfo_path)?;
        memory::parse_meminfo(&raw)
    }

    /// Read current `CPUFreq` policy frequencies, preferring hardware-reported
    /// `cpuinfo_cur_freq` and falling back to `scaling_cur_freq`.
    pub fn cpu_frequency_hz(&self) -> Option<u64> {
        let mut cold_cache = CpuFreqStructuralCache::default();
        self.cpu_frequency_hz_with_cache(&mut cold_cache)
    }

    /// Plan 145: structural `CPUFreq` cache with live online-membership.
    ///
    /// Policy-root enumeration and current-frequency reads stay live every
    /// sample. The cached `related_cpus` identity set is reused only while
    /// the policy directory set is unchanged; the global
    /// `/sys/devices/system/cpu/online` set is read once per sample and
    /// intersected with the cached structural membership, so
    /// same-cardinality online membership changes are immediately visible
    /// without re-reading `related_cpus`. If the global online source is
    /// unavailable, the sample fails closed to the live membership path.
    pub fn cpu_frequency_hz_with_cache(&self, cache: &mut CpuFreqStructuralCache) -> Option<u64> {
        let root = Path::new("/sys/devices/system/cpu/cpufreq");
        let policies = self.inner.read_dir(root).ok()?;
        let mut eligible: Vec<PathBuf> = policies
            .into_iter()
            .filter(|policy| {
                policy
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("policy"))
            })
            .collect();
        eligible.sort();

        // Plan 145: the global online set is the single live dynamic input;
        // a failed read fails closed to the legacy membership path so we
        // never reuse stale dynamic weights.
        let Some(online) = self.read_online_cpus() else {
            cache.online = None;
            cache.related.clear();
            cache.policies.clear();
            return self.cpu_frequency_hz_live_fallback(&eligible);
        };
        cache.online = Some(online.clone());

        // Refresh structural membership only when the policy directory set
        // changes or a previously-unreadable policy becomes readable.
        if cache.policies == eligible {
            for policy in &eligible {
                if !cache.related.contains_key(policy) {
                    if let Some(set) = self.read_related_cpus(policy) {
                        cache.related.insert(policy.clone(), set);
                    }
                }
            }
        } else {
            cache.policies.clone_from(&eligible);
            cache.related.clear();
            for policy in &eligible {
                if let Some(set) = self.read_related_cpus(policy) {
                    cache.related.insert(policy.clone(), set);
                }
            }
        }

        let mut weighted_sum = 0u128;
        let mut weight_sum = 0u128;
        for policy in &eligible {
            let weight = match cache.related.get(policy) {
                Some(related) => CpuFreqStructuralCache::intersection_count(related, &online),
                // Plan 145: per-policy fail-closed fallback when structural
                // membership cannot be read or parsed.
                None => self.cpufreq_policy_weight(policy),
            };
            if weight == 0 {
                continue;
            }
            let Some(khz) = self.cpufreq_current_khz(policy) else {
                continue;
            };
            let Some(hz) = khz.checked_mul(1_000) else {
                continue;
            };
            weighted_sum = weighted_sum.checked_add(u128::from(hz) * weight as u128)?;
            weight_sum = weight_sum.checked_add(weight as u128)?;
        }
        u64::try_from(weighted_sum.checked_div(weight_sum)?).ok()
    }

    /// Plan 145: legacy live-membership fallback.
    ///
    /// Used when the global online source is unreadable so we never reuse
    /// stale dynamic weights after the authoritative input is missing.
    fn cpu_frequency_hz_live_fallback(&self, eligible: &[PathBuf]) -> Option<u64> {
        let mut weighted_sum = 0u128;
        let mut weight_sum = 0u128;
        for policy in eligible {
            let weight = self.cpufreq_policy_weight(policy);
            if weight == 0 {
                continue;
            }
            let Some(khz) = self.cpufreq_current_khz(policy) else {
                continue;
            };
            let Some(hz) = khz.checked_mul(1_000) else {
                continue;
            };
            weighted_sum = weighted_sum.checked_add(u128::from(hz) * weight as u128)?;
            weight_sum = weight_sum.checked_add(weight as u128)?;
        }
        u64::try_from(weighted_sum.checked_div(weight_sum)?).ok()
    }

    /// Plan 145: read the global `/sys/devices/system/cpu/online` set.
    fn read_online_cpus(&self) -> Option<CpuIdSet> {
        let raw = self
            .inner
            .read_to_string(Path::new("/sys/devices/system/cpu/online"))
            .ok()?;
        parse_cpu_list_ids(&raw)
    }

    /// Plan 145: read a policy's structural `related_cpus` membership.
    fn read_related_cpus(&self, policy: &Path) -> Option<CpuIdSet> {
        let raw = self
            .inner
            .read_to_string(&policy.join("related_cpus"))
            .ok()?;
        parse_cpu_list_ids(&raw)
    }

    /// Plan 146: legacy live-membership policy weight.
    ///
    /// Preserves the pre-Plan-141 sequence `affected_cpus -> related_cpus ->
    /// bounded default`, but derives each weight from the parsed identity set
    /// instead of reinterpreting ranges as dense CPU IDs bounded by the host
    /// core count. A policy whose `affected_cpus` cannot be read at all keeps
    /// the existing bounded default weight of 1; a readable but unusable
    /// `affected_cpus` with no usable `related_cpus` contributes 0.
    fn cpufreq_policy_weight(&self, policy: &Path) -> usize {
        let affected = self.inner.read_to_string(&policy.join("affected_cpus"));
        match affected {
            Ok(raw) => parse_cpu_list_ids(&raw)
                .map(|ids| ids.len())
                .or_else(|| self.cpufreq_related_cpus_weight(policy))
                .unwrap_or(0),
            Err(_) => self.cpufreq_related_cpus_weight(policy).unwrap_or(1),
        }
    }

    /// Plan 146: membership weight taken from a policy's `related_cpus`.
    fn cpufreq_related_cpus_weight(&self, policy: &Path) -> Option<usize> {
        let raw = self
            .inner
            .read_to_string(&policy.join("related_cpus"))
            .ok()?;
        parse_cpu_list_ids(&raw).map(|ids| ids.len())
    }

    fn cpufreq_current_khz(&self, policy: &Path) -> Option<u64> {
        self.inner
            .read_to_string(&policy.join("cpuinfo_cur_freq"))
            .ok()
            .and_then(|raw| parse_positive_u64(&raw))
            .or_else(|| {
                self.inner
                    .read_to_string(&policy.join("scaling_cur_freq"))
                    .ok()
                    .and_then(|raw| parse_positive_u64(&raw))
            })
    }

    /// Read top-level Linux block-device sector counters. Partitions are not
    /// enumerated separately, and layered devices with slaves are omitted so
    /// one physical accounting layer is selected deterministically.
    pub fn disk_io(&self) -> Result<Vec<RawDiskIo>, CollectError> {
        let root = Path::new("/sys/block");
        let devices = self.inner.read_dir(root)?;
        let mut records = Vec::new();
        for device in devices {
            let Some(name) = device.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if name.starts_with("loop") || name.starts_with("ram") || name.starts_with("zram") {
                continue;
            }
            // Single `read_dir` attempt: a missing `slaves` directory means
            // a leaf device. A separate `path_exists` probe first would be
            // a TOCTOU pair (the directory can appear/disappear between
            // the two syscalls); any read error is treated as "no slaves".
            if self
                .inner
                .read_dir(&device.join("slaves"))
                .is_ok_and(|slaves| !slaves.is_empty())
            {
                continue;
            }
            let Ok(raw) = self.inner.read_to_string(&device.join("stat")) else {
                continue;
            };
            // Walk fields without a per-line `Vec` allocation.
            let mut fields = raw.split_whitespace();
            let (Some(read_str), Some(write_str)) = (fields.nth(2), fields.nth(3)) else {
                continue;
            };
            let Ok(read_sectors) = read_str.parse::<u64>() else {
                continue;
            };
            let Ok(write_sectors) = write_str.parse::<u64>() else {
                continue;
            };
            // One corrupt counter must skip only its device, never abort the
            // whole list (mirrors the parse-failure arms above).
            let (Some(read_bytes), Some(write_bytes)) = (
                read_sectors.checked_mul(512),
                write_sectors.checked_mul(512),
            ) else {
                continue;
            };
            records.push(RawDiskIo {
                id: name.to_owned(),
                name: name.to_owned(),
                read_bytes,
                write_bytes,
            });
        }
        records.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(records)
    }

    /// Read byte counters and link metadata from procfs/sysfs.
    pub fn network_interfaces(&self) -> Result<Vec<RawNetworkInterface>, CollectError> {
        let raw = self.inner.read_to_string(Path::new("/proc/net/dev"))?;
        let mut records = Vec::new();
        for line in raw.lines().skip(2) {
            let Some((name, values)) = line.split_once(':') else {
                continue;
            };
            let name = name.trim();
            // Walk fields without a per-line `Vec` allocation: field 0 is
            // rx bytes, field 8 is tx bytes.
            let mut fields = values.split_whitespace();
            let (Some(rx_str), Some(tx_str)) = (fields.next(), fields.nth(7)) else {
                continue;
            };
            let (Ok(rx_bytes), Ok(tx_bytes)) = (rx_str.parse(), tx_str.parse()) else {
                continue;
            };
            let path = Path::new("/sys/class/net").join(name);
            let flags = self
                .inner
                .read_to_string(&path.join("flags"))
                .ok()
                .and_then(|value| {
                    u32::from_str_radix(value.trim().trim_start_matches("0x"), 16).ok()
                })
                .unwrap_or(0);
            let is_loopback = flags & 0x8 != 0;
            let rx_capacity_bps =
                parse_link_speed(self.inner.read_to_string(&path.join("speed")).ok());
            let tx_capacity_bps = rx_capacity_bps;
            let operational = self
                .inner
                .read_to_string(&path.join("operstate"))
                .is_ok_and(|state| state.trim() == "up");
            let slave = self.inner.path_exists(&path.join("master"));
            records.push(RawNetworkInterface {
                id: name.to_owned(),
                name: name.to_owned(),
                rx_bytes,
                tx_bytes,
                rx_capacity_bps,
                tx_capacity_bps,
                is_loopback,
                operational,
                aggregate_member: !is_loopback && !slave,
            });
        }
        records.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(records)
    }

    /// Read Linux mount records from `/proc/self/mountinfo`.
    pub fn read_mountinfo(&self) -> Result<String, CollectError> {
        self.read_path(Path::new("/proc/self/mountinfo"))
    }

    /// Read native capacity for one mount point.
    pub fn statvfs(&self, path: &Path) -> Result<RawStatvfs, CollectError> {
        self.inner.statvfs(path)
    }

    /// Read `/etc/os-release`. Missing file yields `Ok(None)` so identity
    /// collection can fall back to a generic Linux identity.
    pub fn read_os_release(&self) -> Result<Option<String>, CollectError> {
        let path = self
            .os_release_override
            .clone()
            .unwrap_or_else(|| PathBuf::from("/etc/os-release"));
        match self.inner.read_to_string(&path) {
            Ok(s) => Ok(Some(s)),
            Err(err) if err.kind == CollectErrorKind::SourceUnavailable => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// Read the kernel name and release from `/proc/sys/kernel/osrelease` and
    /// `/proc/sys/kernel/ostype`.
    pub fn kernel_identity(&self) -> Result<KernelIdentity, CollectError> {
        let sysname = self
            .read_optional("/proc/sys/kernel/ostype")?
            .unwrap_or_else(|| "Linux".to_string());
        let release = self
            .read_optional("/proc/sys/kernel/osrelease")?
            .unwrap_or_else(|| "unknown".to_string());
        Ok(KernelIdentity { sysname, release })
    }

    /// Read the architecture string from `/proc/sys/kernel/arch` or
    /// `/proc/cpuinfo`. Falls back to "unknown" when neither is present.
    pub fn architecture(&self) -> String {
        if let Ok(Some(arch)) = self.read_optional("/proc/sys/kernel/arch") {
            return arch.trim().to_string();
        }
        if let Ok(raw) = self.read_path(Path::new("/proc/cpuinfo")) {
            for line in raw.lines() {
                if let Some(rest) = line.strip_prefix("machine") {
                    let value = rest.trim_start_matches(|c: char| c == ':' || c.is_whitespace());
                    if !value.is_empty() {
                        return value.to_string();
                    }
                }
            }
        }
        "unknown".to_string()
    }

    /// Read the hostname from `/proc/sys/kernel/hostname`.
    pub fn hostname(&self) -> Result<String, CollectError> {
        let raw = self
            .read_path(Path::new("/proc/sys/kernel/hostname"))?
            .trim()
            .to_string();
        if raw.is_empty() {
            return Err(CollectError::new(
                CollectErrorKind::SourceUnavailable,
                "hostname from /proc/sys/kernel/hostname was empty",
            ));
        }
        Ok(raw)
    }

    /// Logical core count, with the cached value preferred over the kernel
    /// hint.
    #[must_use]
    pub fn logical_core_count(&self) -> Option<usize> {
        self.logical_cores
            .or_else(|| self.inner.available_parallelism())
    }

    fn read_path(&self, path: &Path) -> Result<String, CollectError> {
        self.inner.read_to_string(path)
    }

    fn read_optional(&self, path: &str) -> Result<Option<String>, CollectError> {
        match self.inner.read_to_string(Path::new(path)) {
            Ok(s) => Ok(Some(s)),
            Err(err) if err.kind == CollectErrorKind::SourceUnavailable => Ok(None),
            Err(err) => Err(err),
        }
    }
}

/// Live-host source backed by `std::fs` and `std::thread::available_parallelism`.
#[derive(Debug)]
struct HostSource;

#[allow(unsafe_code)]
impl FileSource for HostSource {
    fn read_to_string(&self, path: &Path) -> Result<String, CollectError> {
        match fs::read_to_string(path) {
            Ok(s) => Ok(s),
            Err(err) => Err(map_io_error(path, err)),
        }
    }

    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>, CollectError> {
        fs::read_dir(path)
            .map_err(|err| map_io_error(path, err))?
            .map(|entry| {
                entry
                    .map(|entry| entry.path())
                    .map_err(|err| map_io_error(path, err))
            })
            .collect()
    }

    fn path_exists(&self, path: &Path) -> bool {
        fs::symlink_metadata(path).is_ok()
    }

    fn available_parallelism(&self) -> Option<usize> {
        std::thread::available_parallelism()
            .ok()
            .map(std::num::NonZeroUsize::get)
    }

    fn statvfs(&self, path: &Path) -> Result<RawStatvfs, CollectError> {
        let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
            .map_err(|_| CollectError::new(CollectErrorKind::Parse, "mount path contains NUL"))?;
        let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        // Safety: c_path is NUL-terminated and stat points to writable,
        // correctly sized storage. The return code is checked before reading.
        let result = unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) };
        if result != 0 {
            return Err(CollectError::new(
                CollectErrorKind::SourceUnavailable,
                format!("statvfs failed for {}", path.display()),
            ));
        }
        // Safety: statvfs initialized the structure when it returned success.
        let stat = unsafe { stat.assume_init() };
        Ok(RawStatvfs {
            blocks: stat.f_blocks,
            free_blocks: stat.f_bfree,
            available_blocks: stat.f_bavail,
            fragment_size: stat.f_frsize,
            block_size: stat.f_bsize,
        })
    }
}

fn map_io_error(path: &Path, err: io::Error) -> CollectError {
    use io::ErrorKind;
    let path_display = path.display().to_string();
    match err.kind() {
        ErrorKind::NotFound => CollectError::new(
            CollectErrorKind::SourceUnavailable,
            format!("source file not found: {path_display}"),
        )
        .with_source(err),
        ErrorKind::PermissionDenied => CollectError::new(
            CollectErrorKind::SourceUnavailable,
            format!("source file permission denied: {path_display}"),
        )
        .with_source(err),
        _ => CollectError::new(
            CollectErrorKind::SourceUnavailable,
            format!("source read error: {err}"),
        )
        .with_source(err),
    }
}

/// Plan 141: deterministic source-call accounting.
///
/// Counts fixture reads per absolute path so steady-state versus
/// topology-change behavior can be asserted structurally without timing.
#[cfg(test)]
#[derive(Debug, Clone, Default)]
pub struct CallCounts {
    reads: std::collections::HashMap<PathBuf, usize>,
    dirs: std::collections::HashMap<PathBuf, usize>,
}

#[cfg(test)]
impl CallCounts {
    fn record_read(&mut self, path: &Path) {
        *self.reads.entry(path.to_path_buf()).or_insert(0) += 1;
    }

    fn record_dir(&mut self, path: &Path) {
        *self.dirs.entry(path.to_path_buf()).or_insert(0) += 1;
    }

    /// Total `read_to_string` calls for paths containing `needle`.
    pub fn reads_containing(&self, needle: &str) -> usize {
        self.reads
            .iter()
            .filter(|(path, _)| path.to_string_lossy().contains(needle))
            .map(|(_, count)| *count)
            .sum()
    }

    /// Total `read_dir` calls for exactly `path`.
    pub fn dirs_for(&self, path: &str) -> usize {
        self.dirs.get(Path::new(path)).copied().unwrap_or(0)
    }

    /// Total `read_to_string` calls for exactly `path`.
    pub fn reads_for(&self, path: &str) -> usize {
        self.reads.get(Path::new(path)).copied().unwrap_or(0)
    }
}

/// In-memory fixture source for tests.
///
/// `MemorySource` is the workhorse of source-level tests. Each constructor
/// accepts `(path, content)` pairs and serves them from a map without
/// touching the filesystem. `logical_cores` is supplied by the caller so the
/// collector's fallback logic can be exercised.
#[derive(Debug, Clone, Default)]
pub struct MemorySource {
    files: std::collections::HashMap<PathBuf, String>,
    stats: std::collections::HashMap<PathBuf, RawStatvfs>,
    logical_cores: Option<usize>,
    #[cfg(test)]
    counts: std::sync::Arc<std::sync::Mutex<CallCounts>>,
}

impl MemorySource {
    /// Construct an empty memory source.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add or replace a file entry.
    #[must_use]
    pub fn with_file(mut self, path: impl Into<PathBuf>, content: impl Into<String>) -> Self {
        self.files.insert(path.into(), content.into());
        self
    }

    /// Add or replace a file entry on an already-constructed source. Used by
    /// tests that share a [`ProcSource`] between fixtures.
    pub fn add_file(&mut self, path: impl Into<PathBuf>, content: impl Into<String>) {
        self.files.insert(path.into(), content.into());
    }

    /// Remove a file entry on an already-constructed source. Used by tests
    /// that simulate a policy disappearing between samples.
    pub fn remove_file(&mut self, path: impl Into<PathBuf>) -> bool {
        self.files.remove(&path.into()).is_some()
    }

    /// Add native filesystem statistics for a fixture mount point.
    pub fn add_statvfs(&mut self, path: impl Into<PathBuf>, stats: RawStatvfs) {
        self.stats.insert(path.into(), stats);
    }

    /// Returns `true` if the given path has been registered as a fixture.
    #[must_use]
    pub fn has_file(&self, path: &str) -> bool {
        self.files.contains_key(Path::new(path))
    }

    /// Set the logical core count returned by [`Self::available_parallelism`].
    #[must_use]
    pub fn with_logical_cores(mut self, cores: usize) -> Self {
        self.logical_cores = Some(cores);
        self
    }

    /// Replace the logical core count on an already-constructed source. Used
    /// by tests that simulate CPU hotplug between samples.
    pub fn set_logical_cores(&mut self, cores: usize) {
        self.logical_cores = Some(cores);
    }

    /// Plan 141: snapshot of deterministic fixture call counts.
    #[cfg(test)]
    #[must_use]
    pub fn call_counts(&self) -> CallCounts {
        self.counts
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }
}

impl FileSource for MemorySource {
    fn read_to_string(&self, path: &Path) -> Result<String, CollectError> {
        #[cfg(test)]
        if let Ok(mut guard) = self.counts.lock() {
            guard.record_read(path);
        }
        if let Some(content) = self.files.get(path) {
            Ok(content.clone())
        } else {
            let display = path.display().to_string();
            Err(CollectError::new(
                CollectErrorKind::SourceUnavailable,
                format!("fixture missing: {display}"),
            ))
        }
    }

    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>, CollectError> {
        #[cfg(test)]
        if let Ok(mut guard) = self.counts.lock() {
            guard.record_dir(path);
        }
        let mut children = std::collections::BTreeSet::new();
        for candidate in self.files.keys() {
            if let Ok(relative) = candidate.strip_prefix(path) {
                if let Some(first) = relative.components().next() {
                    children.insert(path.join(first.as_os_str()));
                }
            }
        }
        if children.is_empty() {
            Err(CollectError::new(
                CollectErrorKind::SourceUnavailable,
                format!("fixture directory missing: {}", path.display()),
            ))
        } else {
            Ok(children.into_iter().collect())
        }
    }

    fn path_exists(&self, path: &Path) -> bool {
        self.files.contains_key(path)
            || self
                .files
                .keys()
                .any(|candidate| candidate.starts_with(path))
    }

    fn available_parallelism(&self) -> Option<usize> {
        self.logical_cores
    }

    fn statvfs(&self, path: &Path) -> Result<RawStatvfs, CollectError> {
        self.stats.get(path).copied().ok_or_else(|| {
            CollectError::new(
                CollectErrorKind::SourceUnavailable,
                format!("fixture statvfs missing: {}", path.display()),
            )
        })
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any>
    where
        Self: 'static,
    {
        Some(self)
    }
}

/// Owned subset of Linux `statvfs` needed for capacity arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawStatvfs {
    pub blocks: u64,
    pub free_blocks: u64,
    pub available_blocks: u64,
    pub fragment_size: u64,
    pub block_size: u64,
}

/// Cumulative Linux block-device byte counters after sector conversion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawDiskIo {
    pub id: String,
    pub name: String,
    pub read_bytes: u64,
    pub write_bytes: u64,
}

/// Cumulative Linux interface counters and native link metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawNetworkInterface {
    pub id: String,
    pub name: String,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_capacity_bps: Option<u64>,
    pub tx_capacity_bps: Option<u64>,
    pub is_loopback: bool,
    pub operational: bool,
    pub aggregate_member: bool,
}

fn parse_positive_u64(raw: &str) -> Option<u64> {
    let value = raw.trim().parse::<u64>().ok()?;
    (value > 0).then_some(value)
}

fn parse_link_speed(raw: Option<String>) -> Option<u64> {
    let mbps = raw?.trim().parse::<u64>().ok()?;
    (mbps > 0).then(|| mbps.checked_mul(1_000_000)).flatten()
}

/// Plan 146: parse a Linux CPU list into a bounded set of CPU identities.
///
/// Accepts the kernel cpulist grammar Gregg already supports: single IDs,
/// comma- or whitespace-separated values, and inclusive ranges. CPU numbers
/// are identities rather than dense positions, so each ID is preserved
/// exactly; nothing is remapped, clamped, or otherwise interpreted as a
/// count-derived index.
///
/// The list is accepted only when it yields at most
/// [`MAX_CPU_SET_MEMBERS`] distinct identities. The whole list is rejected
/// (never silently truncated) when a token or range is reversed, when an ID
/// overflows `u32`, when the input is empty or malformed, or when adding a
/// token or range would exceed the member bound. An oversized declared
/// range is rejected from its own span before any member is materialized, so
/// cost is bounded by the accepted cardinality rather than by an
/// attacker-controlled byte length.
fn parse_cpu_list_ids(raw: &str) -> Option<CpuIdSet> {
    let mut ids = BTreeSet::new();
    let bound = MAX_CPU_SET_MEMBERS as u64;
    for item in raw
        .trim()
        .split(|character: char| character == ',' || character.is_whitespace())
        .filter(|item| !item.is_empty())
    {
        let (start, end) = item.split_once('-').map_or_else(
            || item.parse::<u32>().ok().map(|value| (value, value)),
            |(start, end)| Some((start.parse().ok()?, end.parse().ok()?)),
        )?;
        if end < start {
            return None;
        }
        let span = u64::from(end - start) + 1;
        // Reject an oversized declared range from its span alone, before
        // any iteration or allocation proportional to it.
        if span > bound {
            return None;
        }
        if ids.len() as u64 + span <= bound {
            ids.extend(start..=end);
            continue;
        }
        // The range overlaps existing members, so count only the identities
        // it would actually add: repeated IDs and ranges must neither inflate
        // nor bypass the member bound.
        let fresh = span - ids.range(start..=end).count() as u64;
        if fresh == 0 {
            continue;
        }
        if ids.len() as u64 + fresh > bound {
            return None;
        }
        ids.extend(start..=end);
    }
    (!ids.is_empty()).then_some(CpuIdSet { ids })
}

/// Output of [`ProcSource::read_proc_stat`].
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ParsedProcStat {
    /// Aggregate `cpu` line counters, if present. `None` if `/proc/stat` is
    /// missing the canonical `cpu` row.
    pub aggregate: Option<crate::linux::CpuCounters>,
}

/// Output of [`ProcSource::read_proc_meminfo`].
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ParsedMeminfo {
    pub mem_total_kb: Option<u64>,
    pub mem_available_kb: Option<u64>,
    pub mem_free_kb: Option<u64>,
    pub buffers_kb: Option<u64>,
    pub cached_kb: Option<u64>,
    pub s_reclaimable_kb: Option<u64>,
    pub swap_total_kb: Option<u64>,
    pub swap_free_kb: Option<u64>,
}

/// Output of [`ProcSource::kernel_identity`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelIdentity {
    pub sysname: String,
    pub release: String,
}

// Pull the cpu and memory submodules in so the public helpers referenced
// above are defined.
use crate::linux::{cpu, memory};

#[cfg(test)]
mod tests {
    use super::*;

    fn source_with(files: &[(&str, &str)]) -> ProcSource {
        let mut source = MemorySource::new().with_logical_cores(4);
        for (path, content) in files {
            source.add_file(*path, *content);
        }
        ProcSource::for_memory(source)
    }

    #[test]
    fn cpufreq_prefers_hardware_current_and_weights_policy_membership() {
        let source = source_with(&[
            (
                "/sys/devices/system/cpu/cpufreq/policy0/affected_cpus",
                "0-1\n",
            ),
            (
                "/sys/devices/system/cpu/cpufreq/policy0/cpuinfo_cur_freq",
                "2000000\n",
            ),
            (
                "/sys/devices/system/cpu/cpufreq/policy1/affected_cpus",
                "2-3\n",
            ),
            (
                "/sys/devices/system/cpu/cpufreq/policy1/cpuinfo_cur_freq",
                "1000000\n",
            ),
        ]);
        assert_eq!(source.cpu_frequency_hz(), Some(1_500_000_000));
    }

    #[test]
    fn cpufreq_falls_back_to_scaling_current_and_ignores_bad_policy() {
        let source = source_with(&[
            (
                "/sys/devices/system/cpu/cpufreq/policy0/affected_cpus",
                "0\n",
            ),
            (
                "/sys/devices/system/cpu/cpufreq/policy0/cpuinfo_cur_freq",
                "not-a-frequency\n",
            ),
            (
                "/sys/devices/system/cpu/cpufreq/policy0/scaling_cur_freq",
                "1800000\n",
            ),
            (
                "/sys/devices/system/cpu/cpufreq/policy1/affected_cpus",
                "1\n",
            ),
            (
                "/sys/devices/system/cpu/cpufreq/policy1/cpuinfo_cur_freq",
                "0\n",
            ),
        ]);
        assert_eq!(source.cpu_frequency_hz(), Some(1_800_000_000));
    }

    #[test]
    fn cpufreq_accepts_kernel_space_separated_cpu_lists() {
        let source = source_with(&[
            (
                "/sys/devices/system/cpu/cpufreq/policy0/affected_cpus",
                "0 1 2 3\n",
            ),
            (
                "/sys/devices/system/cpu/cpufreq/policy0/scaling_cur_freq",
                "2100000\n",
            ),
        ]);
        assert_eq!(source.cpu_frequency_hz(), Some(2_100_000_000));
    }

    #[test]
    fn disk_stats_convert_sectors_and_select_one_layer() {
        let source = source_with(&[
            ("/sys/block/sda/stat", "1 2 10 4 5 6 20 8 9 10 11\n"),
            ("/sys/block/dm-0/stat", "1 2 100 4 5 6 200 8 9 10 11\n"),
            ("/sys/block/dm-0/slaves/sda", "\n"),
        ]);
        let disks = source.disk_io().expect("disk fixtures");
        assert_eq!(disks.len(), 1);
        assert_eq!(disks[0].read_bytes, 5_120);
        assert_eq!(disks[0].write_bytes, 10_240);
    }

    #[test]
    fn network_keeps_loopback_detail_and_excludes_slave_capacity() {
        let source = source_with(&[
            ("/proc/net/dev", "Inter-| Receive | Transmit\n face |bytes packets errs drop fifo frame compressed multicast |bytes packets errs drop fifo colls carrier compressed\nlo: 100 0 0 0 0 0 0 0 200 0 0 0 0 0 0 0\neth0: 300 0 0 0 0 0 0 0 400 0 0 0 0 0 0 0\neth1: 500 0 0 0 0 0 0 0 600 0 0 0 0 0 0 0\n"),
            ("/sys/class/net/lo/flags", "0x9\n"),
            ("/sys/class/net/lo/operstate", "unknown\n"),
            ("/sys/class/net/eth0/flags", "0x1\n"),
            ("/sys/class/net/eth0/operstate", "up\n"),
            ("/sys/class/net/eth0/speed", "1000\n"),
            ("/sys/class/net/eth1/flags", "0x1\n"),
            ("/sys/class/net/eth1/operstate", "down\n"),
            ("/sys/class/net/eth1/speed", "1000\n"),
            ("/sys/class/net/eth1/master", "\n"),
        ]);
        let interfaces = source.network_interfaces().expect("network fixtures");
        assert_eq!(interfaces.len(), 3);
        assert!(
            interfaces
                .iter()
                .find(|i| i.id == "lo")
                .unwrap()
                .is_loopback
        );
        assert!(
            interfaces
                .iter()
                .find(|i| i.id == "eth0")
                .unwrap()
                .aggregate_member
        );
        assert!(
            !interfaces
                .iter()
                .find(|i| i.id == "eth1")
                .unwrap()
                .aggregate_member
        );
        assert_eq!(
            interfaces
                .iter()
                .find(|i| i.id == "eth0")
                .unwrap()
                .rx_capacity_bps,
            Some(1_000_000_000)
        );
    }

    // ===== Plan 141: source-call accounting and CPUFreq structural cache =====

    #[test]
    fn plan141_source_call_accounting_covers_hot_paths() {
        let mut mem = MemorySource::new().with_logical_cores(4);
        for (path, content) in [
            ("/proc/stat", "cpu  100 0 50 8000 30 5 2 1 0 0\n"),
            ("/proc/loadavg", "0.10 0.20 0.30 1/50 1\n"),
            (
                "/proc/meminfo",
                "MemTotal:        8000000 kB\nMemAvailable:     4000000 kB\n",
            ),
            (
                "/sys/devices/system/cpu/cpufreq/policy0/affected_cpus",
                "0-1\n",
            ),
            (
                "/sys/devices/system/cpu/cpufreq/policy0/cpuinfo_cur_freq",
                "2000000\n",
            ),
            ("/sys/block/sda/stat", "1 2 10 4 5 6 20 8 9 10 11\n"),
            ("/proc/net/dev", "Inter-| Receive | Transmit\n face |bytes packets errs drop fifo frame compressed multicast |bytes packets errs drop fifo colls carrier compressed\neth0: 300 0 0 0 0 0 0 0 400 0 0 0 0 0 0 0\n"),
            ("/sys/class/net/eth0/flags", "0x1\n"),
            ("/sys/class/net/eth0/operstate", "up\n"),
            ("/sys/class/net/eth0/speed", "1000\n"),
        ] {
            mem.add_file(path, content);
        }
        let probe = mem.clone();
        let source = ProcSource::for_memory(mem);
        let _ = source.read_proc_stat();
        let _ = source.read_proc_loadavg();
        let _ = source.read_proc_meminfo();
        let _ = source.cpu_frequency_hz();
        let _ = source.disk_io();
        let _ = source.network_interfaces();
        let counts = probe.call_counts();
        assert_eq!(counts.reads_for("/proc/stat"), 1);
        assert_eq!(counts.reads_for("/proc/loadavg"), 1);
        assert_eq!(counts.reads_for("/proc/meminfo"), 1);
        assert_eq!(
            counts.dirs_for("/sys/devices/system/cpu/cpufreq"),
            1,
            "CPUFreq root enumeration must be accounted"
        );
        assert!(counts.reads_containing("affected_cpus") >= 1);
        assert!(counts.reads_containing("cpuinfo_cur_freq") >= 1);
        assert_eq!(counts.dirs_for("/sys/block"), 1);
        assert!(counts.reads_containing("/sys/block/sda/stat") >= 1);
        assert_eq!(counts.reads_for("/proc/net/dev"), 1);
        assert!(counts.reads_containing("/sys/class/net/eth0/flags") >= 1);
    }

    #[test]
    fn plan141_steady_cpufreq_avoids_membership_reads_but_stays_live() {
        // Rebuild the CPUFreq fixture here with a shared probe so call
        // counts stay observable (`source_with` moves its map).
        // `plan141_cpufreq_source` documents the canonical fixture shape.
        let mut mem = MemorySource::new().with_logical_cores(4);
        for (path, content) in [
            ("/sys/devices/system/cpu/online", "0-3\n"),
            (
                "/sys/devices/system/cpu/cpufreq/policy0/related_cpus",
                "0-1\n",
            ),
            (
                "/sys/devices/system/cpu/cpufreq/policy0/cpuinfo_cur_freq",
                "2000000\n",
            ),
            (
                "/sys/devices/system/cpu/cpufreq/policy1/related_cpus",
                "2-3\n",
            ),
            (
                "/sys/devices/system/cpu/cpufreq/policy1/cpuinfo_cur_freq",
                "1000000\n",
            ),
        ] {
            mem.add_file(path, content);
        }
        let probe = mem.clone();
        let source = ProcSource::for_memory(mem);
        let mut cache = CpuFreqStructuralCache::default();
        let first = source.cpu_frequency_hz_with_cache(&mut cache);
        assert_eq!(first, Some(1_500_000_000));
        let after_first = probe.call_counts();
        let related_first = after_first.reads_containing("related_cpus");
        let freq_first = after_first.reads_containing("cpuinfo_cur_freq")
            + after_first.reads_containing("scaling_cur_freq");
        let online_first = after_first.reads_for("/sys/devices/system/cpu/online");
        assert!(related_first >= 2);
        assert!(freq_first >= 2);
        assert!(online_first >= 1, "online set must be read each sample");

        // Steady topology: membership files must not be re-read while
        // current frequency and online membership stay live.
        let second = source.cpu_frequency_hz_with_cache(&mut cache);
        assert_eq!(second, first);
        let after_second = probe.call_counts();
        let related_second = after_second.reads_containing("related_cpus");
        let freq_second = after_second.reads_containing("cpuinfo_cur_freq")
            + after_second.reads_containing("scaling_cur_freq");
        let online_second = after_second.reads_for("/sys/devices/system/cpu/online");
        assert_eq!(
            related_second, related_first,
            "steady state must avoid repeated structural membership reads"
        );
        assert!(
            freq_second > freq_first,
            "current frequency must remain sampled every cycle"
        );
        assert!(
            online_second > online_first,
            "online set must be re-read every sample to drive policy weights"
        );
        // Root enumeration stays live for immediate policy visibility.
        assert!(after_second.dirs_for("/sys/devices/system/cpu/cpufreq") >= 2);
    }

    #[test]
    fn plan141_cpufreq_policy_and_core_changes_force_refresh() {
        let mut mem = MemorySource::new().with_logical_cores(4);
        mem.add_file("/sys/devices/system/cpu/online", "0-3\n");
        mem.add_file(
            "/sys/devices/system/cpu/cpufreq/policy0/related_cpus",
            "0-1\n",
        );
        mem.add_file(
            "/sys/devices/system/cpu/cpufreq/policy0/cpuinfo_cur_freq",
            "2000000\n",
        );
        mem.add_file(
            "/sys/devices/system/cpu/cpufreq/policy1/related_cpus",
            "2-3\n",
        );
        mem.add_file(
            "/sys/devices/system/cpu/cpufreq/policy1/cpuinfo_cur_freq",
            "1000000\n",
        );
        let mut source = ProcSource::for_memory(mem);
        let mut cache = CpuFreqStructuralCache::default();
        let first = source.cpu_frequency_hz_with_cache(&mut cache);
        assert_eq!(first, Some(1_500_000_000));

        // Policy add/remove is visible immediately and matches a cold query.
        source.memory_source_mut().expect("memory source").add_file(
            "/sys/devices/system/cpu/cpufreq/policy2/related_cpus",
            "0-3\n",
        );
        source.memory_source_mut().expect("memory source").add_file(
            "/sys/devices/system/cpu/cpufreq/policy2/cpuinfo_cur_freq",
            "3000000\n",
        );
        let after_add = source.cpu_frequency_hz_with_cache(&mut cache);
        let cold_after_add = {
            let mut cold = CpuFreqStructuralCache::default();
            source.cpu_frequency_hz_with_cache(&mut cold)
        };
        // Cold comparison uses a fresh cache on the same fixture; both must
        // observe the new policy without a stale window.
        assert_eq!(after_add, cold_after_add);

        // Online-set change is reflected in the next sample without
        // structural membership refresh.
        source
            .memory_source_mut()
            .expect("memory source")
            .add_file("/sys/devices/system/cpu/online", "0-1\n");
        let after_online_change = source.cpu_frequency_hz_with_cache(&mut cache);
        let mut cold_online = CpuFreqStructuralCache::default();
        let cold_online_value = source.cpu_frequency_hz_with_cache(&mut cold_online);
        assert_eq!(after_online_change, cold_online_value);
    }

    #[test]
    fn plan141_once_lock_pattern_retries_failure_then_caches_success() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        let cell: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
        let read_once = || -> Result<u64, &'static str> {
            let attempt = CALLS.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                return Err("first query fails");
            }
            if let Some(cached) = cell.get() {
                return Ok(*cached);
            }
            let fresh = 16_384u64;
            let _ = cell.set(fresh);
            Ok(*cell.get().unwrap_or(&fresh))
        };
        assert!(read_once().is_err(), "failed first query must be retryable");
        assert_eq!(read_once(), Ok(16_384));
        let calls_after_success = CALLS.load(Ordering::SeqCst);
        assert_eq!(read_once(), Ok(16_384));
        assert_eq!(
            CALLS.load(Ordering::SeqCst),
            calls_after_success + 1,
            "cached success must not re-run FFI, only the wrapper call counts"
        );
        assert_eq!(cell.get(), Some(&16_384));
    }

    // ===== Plan 145: CPUFreq online-membership freshness =====

    fn cpufreq_two_policy_fixture() -> MemorySource {
        let mut mem = MemorySource::new().with_logical_cores(4);
        // Policies with overlapping related membership and divergent
        // frequencies so weight changes change the weighted average.
        mem.add_file(
            "/sys/devices/system/cpu/cpufreq/policy0/related_cpus",
            "0-2\n",
        );
        mem.add_file(
            "/sys/devices/system/cpu/cpufreq/policy0/cpuinfo_cur_freq",
            "1000000\n",
        );
        mem.add_file(
            "/sys/devices/system/cpu/cpufreq/policy1/related_cpus",
            "0-1,3\n",
        );
        mem.add_file(
            "/sys/devices/system/cpu/cpufreq/policy1/cpuinfo_cur_freq",
            "3000000\n",
        );
        mem
    }

    #[test]
    fn plan145_same_count_membership_swap_changes_weighted_average() {
        let mut mem = cpufreq_two_policy_fixture();
        mem.add_file("/sys/devices/system/cpu/online", "0-1\n");
        let mut source = ProcSource::for_memory(mem);
        let mut cache = CpuFreqStructuralCache::default();

        let first = source
            .cpu_frequency_hz_with_cache(&mut cache)
            .expect("weighted hz");
        // online = {0,1}; policy0 related ∩ online = {0,1}, weight 2;
        // policy1 related ∩ online = {0,1}, weight 2;
        // weighted = (1e9*2 + 3e9*2) / 4 = 2_000_000_000
        assert_eq!(first, 2_000_000_000);

        let after_first = source
            .memory_source_mut()
            .expect("memory source")
            .call_counts();
        let related_first = after_first.reads_containing("related_cpus");

        // Same-cardinality swap: online moves to {0,2}. The policy set
        // and `related_cpus` membership are unchanged.
        source
            .memory_source_mut()
            .expect("memory source")
            .add_file("/sys/devices/system/cpu/online", "0,2\n");
        let second = source
            .cpu_frequency_hz_with_cache(&mut cache)
            .expect("weighted hz");
        let after_second = source
            .memory_source_mut()
            .expect("memory source")
            .call_counts();
        let related_second = after_second.reads_containing("related_cpus");

        // online = {0,2}; policy0 related ∩ online = {0,2}, weight 2;
        // policy1 related ∩ online = {0}, weight 1;
        // weighted = (1e9*2 + 3e9*1) / 3 = 1_666_666_666
        assert_eq!(second, 1_666_666_666);
        assert_ne!(
            first, second,
            "same-cardinality swap must change the weighted average"
        );
        assert_eq!(
            related_second, related_first,
            "structural membership must not be re-read for the swap"
        );
        assert!(
            after_second.reads_for("/sys/devices/system/cpu/online")
                > after_first.reads_for("/sys/devices/system/cpu/online"),
            "online set must be re-read every sample"
        );
    }

    #[test]
    fn plan145_ordinary_steady_state_keeps_live_current_freq_and_online() {
        let mut mem = cpufreq_two_policy_fixture();
        mem.add_file("/sys/devices/system/cpu/online", "0-3\n");
        let probe = mem.clone();
        let source = ProcSource::for_memory(mem);
        let mut cache = CpuFreqStructuralCache::default();

        // First sample seeds the cache.
        let first = source
            .cpu_frequency_hz_with_cache(&mut cache)
            .expect("weighted hz");
        let after_first = probe.call_counts();
        let related_first = after_first.reads_containing("related_cpus");
        let online_first = after_first.reads_for("/sys/devices/system/cpu/online");
        let freq_first = after_first.reads_containing("cpuinfo_cur_freq");

        // Second sample: same topology, same online, same frequencies.
        let second = source
            .cpu_frequency_hz_with_cache(&mut cache)
            .expect("weighted hz");
        assert_eq!(second, first);
        let after_second = probe.call_counts();
        let related_second = after_second.reads_containing("related_cpus");
        let online_second = after_second.reads_for("/sys/devices/system/cpu/online");
        let freq_second = after_second.reads_containing("cpuinfo_cur_freq");

        assert_eq!(
            related_second, related_first,
            "structural membership must not advance in steady state"
        );
        assert!(
            online_second > online_first,
            "online must be re-read every sample"
        );
        assert!(
            freq_second > freq_first,
            "current frequency must be sampled every cycle"
        );
        assert!(
            after_second.reads_containing("affected_cpus")
                == after_first.reads_containing("affected_cpus"),
            "no dynamic membership reads must occur when the cache and online set are healthy"
        );
    }

    #[test]
    fn plan145_policy_topology_change_invalidates_structural_cache() {
        let mut mem = cpufreq_two_policy_fixture();
        mem.add_file("/sys/devices/system/cpu/online", "0-3\n");
        let mut source = ProcSource::for_memory(mem);
        let mut cache = CpuFreqStructuralCache::default();

        let first = source
            .cpu_frequency_hz_with_cache(&mut cache)
            .expect("weighted hz");

        // Add a new policy path.
        source.memory_source_mut().expect("memory source").add_file(
            "/sys/devices/system/cpu/cpufreq/policy2/related_cpus",
            "0-3\n",
        );
        source.memory_source_mut().expect("memory source").add_file(
            "/sys/devices/system/cpu/cpufreq/policy2/cpuinfo_cur_freq",
            "5000000\n",
        );

        let after_add = source
            .cpu_frequency_hz_with_cache(&mut cache)
            .expect("weighted hz");
        let cold = {
            let mut cold = CpuFreqStructuralCache::default();
            source.cpu_frequency_hz_with_cache(&mut cold)
        };
        // Online = {0,1,2,3}; policy0 related = {0,1,2}, weight 3;
        // policy1 related = {0,1,3}, weight 3; policy2 related = {0..3}, weight 4.
        // weighted = (1*3 + 3*3 + 5*4) / (3+3+4) = (3 + 9 + 20) / 10 = 3_200_000_000
        assert_eq!(after_add, 3_200_000_000);
        assert_eq!(
            after_add,
            cold.expect("cold value"),
            "structural cache must match a cold query after a topology change"
        );
        assert_ne!(after_add, first);

        // Remove a policy by deleting its directory entry: the cache must
        // refresh and yield the same value as a cold query.
        source
            .memory_source_mut()
            .expect("memory source")
            .remove_file("/sys/devices/system/cpu/cpufreq/policy2/related_cpus");
        source
            .memory_source_mut()
            .expect("memory source")
            .remove_file("/sys/devices/system/cpu/cpufreq/policy2/cpuinfo_cur_freq");
        let after_remove = source
            .cpu_frequency_hz_with_cache(&mut cache)
            .expect("weighted hz");
        let mut cold2 = CpuFreqStructuralCache::default();
        let cold_remove = source.cpu_frequency_hz_with_cache(&mut cold2);
        assert_eq!(after_remove, cold_remove.expect("cold remove"));
        assert_eq!(after_remove, first);
    }

    #[test]
    fn plan145_online_set_failure_falls_back_to_live_membership() {
        let mut mem = cpufreq_two_policy_fixture();
        mem.add_file("/sys/devices/system/cpu/online", "0-3\n");
        let mut source = ProcSource::for_memory(mem);
        let mut cache = CpuFreqStructuralCache::default();
        let first = source
            .cpu_frequency_hz_with_cache(&mut cache)
            .expect("weighted hz");
        // Populated cache proves the structural path was used.
        assert!(cache.online.is_some());

        // Malformed online content forces the fallback path. The fallback
        // must not reuse stale dynamic weights from the cache.
        source
            .memory_source_mut()
            .expect("memory source")
            .add_file("/sys/devices/system/cpu/online", "not a cpu list\n");
        let fallback = source
            .cpu_frequency_hz_with_cache(&mut cache)
            .expect("fallback weighted hz");
        let live_cold = {
            let mut cold = CpuFreqStructuralCache::default();
            source.cpu_frequency_hz_with_cache(&mut cold)
        };
        assert_eq!(fallback, live_cold.expect("live cold"));
        assert_eq!(fallback, first, "fallback must read affected_cpus live");

        // Cache state must be cleared so a stale online cannot re-enter
        // the structural path until the source is readable again.
        assert!(cache.online.is_none());

        // Restoring the online file must succeed again and re-populate the cache.
        source
            .memory_source_mut()
            .expect("memory source")
            .add_file("/sys/devices/system/cpu/online", "0-3\n");
        let recovered = source
            .cpu_frequency_hz_with_cache(&mut cache)
            .expect("recovered weighted hz");
        assert_eq!(recovered, first);
        assert!(cache.online.is_some());
    }

    #[test]
    fn plan145_zero_online_policy_does_not_distort_weighted_average() {
        let mut mem = MemorySource::new().with_logical_cores(4);
        mem.add_file("/sys/devices/system/cpu/online", "0-1\n");
        mem.add_file(
            "/sys/devices/system/cpu/cpufreq/policy0/related_cpus",
            "0-1\n",
        );
        mem.add_file(
            "/sys/devices/system/cpu/cpufreq/policy0/cpuinfo_cur_freq",
            "1000000\n",
        );
        // policy1's structural CPUs are all offline under the live set.
        mem.add_file(
            "/sys/devices/system/cpu/cpufreq/policy1/related_cpus",
            "2-3\n",
        );
        mem.add_file(
            "/sys/devices/system/cpu/cpufreq/policy1/cpuinfo_cur_freq",
            "5000000\n",
        );
        let source = ProcSource::for_memory(mem);
        let mut cache = CpuFreqStructuralCache::default();
        let value = source
            .cpu_frequency_hz_with_cache(&mut cache)
            .expect("weighted hz");
        // Only policy0 contributes (weight 2); policy1 contributes 0.
        assert_eq!(value, 1_000_000_000);
        // The structural entry remains cached for policy1 so its identity
        // is preserved when the online set changes.
        assert_eq!(cache.related.len(), 2);
    }

    #[test]
    fn plan145_per_policy_related_cpus_failure_uses_legacy_weight() {
        let mut mem = cpufreq_two_policy_fixture();
        mem.add_file("/sys/devices/system/cpu/online", "0-3\n");
        // Drop policy0's related_cpus so the cache cannot use it; the
        // legacy per-policy path reads affected_cpus.
        mem.remove_file("/sys/devices/system/cpu/cpufreq/policy0/related_cpus");
        mem.add_file(
            "/sys/devices/system/cpu/cpufreq/policy0/affected_cpus",
            "0-2\n",
        );
        let source = ProcSource::for_memory(mem);
        let mut cache = CpuFreqStructuralCache::default();
        let value = source
            .cpu_frequency_hz_with_cache(&mut cache)
            .expect("weighted hz");
        // policy0 weight from affected_cpus = 3, freq 1e9;
        // policy1 weight = |{0,1,3} ∩ {0,1,2,3}| = 3, freq 3e9;
        // weighted = (1*3 + 3*3) / 6 = 12 / 6 = 2_000_000_000
        assert_eq!(value, 2_000_000_000);
    }

    #[test]
    fn plan145_cpulist_parser_supports_single_id_comma_range_whitespace() {
        let ids = parse_cpu_list_ids("0-3\n").expect("parseable");
        assert_eq!(ids.len(), 4);
        assert!(ids.contains(0));
        assert!(ids.contains(3));

        let comma = parse_cpu_list_ids("0,2,4").expect("parseable");
        assert_eq!(comma.len(), 3);
        assert!(comma.contains(0) && comma.contains(2) && comma.contains(4));

        let whitespace = parse_cpu_list_ids("0 1 2 3").expect("parseable");
        assert_eq!(whitespace.len(), 4);
        assert_eq!(whitespace.iter().collect::<Vec<_>>(), vec![0, 1, 2, 3]);

        let mixed = parse_cpu_list_ids("0-1,3 5-6").expect("parseable");
        assert_eq!(mixed.len(), 5);
        assert!(mixed.contains(1) && mixed.contains(3) && mixed.contains(6));

        assert!(parse_cpu_list_ids("").is_none());
        assert!(parse_cpu_list_ids("not a cpu list").is_none());
        assert!(parse_cpu_list_ids("5-2").is_none());
    }

    // ===== Plan 146: bounded CPU-set cardinality boundary =====

    fn dense_range_list(start: u32, end: u32) -> String {
        (start..=end)
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(",")
    }

    #[test]
    fn plan146_cpulist_accepts_exactly_max_members_and_rejects_one_more() {
        let at_bound = parse_cpu_list_ids(&dense_range_list(0, 8_191)).expect("bounded accept");
        assert_eq!(at_bound.len(), MAX_CPU_SET_MEMBERS);

        let over_bound = parse_cpu_list_ids(&dense_range_list(0, 8_192));
        assert!(
            over_bound.is_none(),
            "8193 distinct identities must fail instead of truncating"
        );
    }

    #[test]
    fn plan146_cpulist_preserves_sparse_high_cpu_identities() {
        let sparse = parse_cpu_list_ids("0,2,10000").expect("sparse list parses");
        assert_eq!(sparse.len(), 3);
        assert_eq!(sparse.iter().collect::<Vec<_>>(), vec![0, 2, 10_000]);

        let mixed = parse_cpu_list_ids("0-1,3 5-6,1000000").expect("mixed list parses");
        assert_eq!(mixed.len(), 6);
        assert!(mixed.contains(1) && mixed.contains(3) && mixed.contains(6));
        assert!(mixed.contains(1_000_000));

        // The largest representable Linux CPU index is still one identity,
        // so it is accepted rather than treated as an oversized declaration.
        let highest = parse_cpu_list_ids(&u32::MAX.to_string()).expect("u32::MAX is one id");
        assert_eq!(highest.len(), 1);
        assert!(highest.contains(u32::MAX));
    }

    #[test]
    fn plan146_cpulist_rejects_oversized_range_without_materializing_members() {
        // A declared range far beyond the member bound fails from its span,
        // so no 8192/8193-member partial set is ever constructed.
        assert!(parse_cpu_list_ids("0-999999").is_none());
        assert!(parse_cpu_list_ids(&format!("0-{},5", u32::MAX)).is_none());
        assert!(parse_cpu_list_ids(&format!("{}-{}", u32::MAX - 100_000, u32::MAX)).is_none());

        // A range that fits alone but overflows the bound when combined with
        // the identities already parsed is rejected atomically.
        let at_bound = parse_cpu_list_ids(&dense_range_list(0, 8_191)).expect("bounded accept");
        assert_eq!(at_bound.len(), MAX_CPU_SET_MEMBERS);
        assert!(parse_cpu_list_ids("0-8191,1000000").is_none());
        assert!(parse_cpu_list_ids("0-8191,1-8192").is_none());
    }

    #[test]
    fn plan146_cpulist_rejects_reversed_overflow_and_malformed_input() {
        assert!(parse_cpu_list_ids("5-2").is_none());
        assert!(parse_cpu_list_ids("0-1,4-3").is_none());
        // Values beyond the ordinary Linux CPU index width are rejected
        // instead of wrapping.
        assert!(parse_cpu_list_ids(&(u64::from(u32::MAX) + 1).to_string()).is_none());
        assert!(parse_cpu_list_ids("99999999999999999999").is_none());
        assert!(parse_cpu_list_ids("0-99999999999999999999").is_none());
        assert!(parse_cpu_list_ids("-1").is_none());
        assert!(parse_cpu_list_ids("0-").is_none());
        assert!(parse_cpu_list_ids(",,,").is_none());
        assert!(parse_cpu_list_ids("   \n  ").is_none());
        // Empty separators stay tolerated exactly as before: they are not
        // identities, so `0,,1` is still the two-member list `0,1`.
        assert_eq!(parse_cpu_list_ids("0,,1").expect("tolerated").len(), 2);
    }

    #[test]
    fn plan146_cpulist_duplicates_count_distinct_members_only() {
        let repeated = parse_cpu_list_ids("0-3,0-3,2,2-3").expect("duplicates parse");
        assert_eq!(repeated.len(), 4);
        assert_eq!(repeated.iter().collect::<Vec<_>>(), vec![0, 1, 2, 3]);

        // Repeating the full bound keeps the distinct cardinality inside the
        // limit instead of being rejected as inflated declared membership.
        let repeated_bound =
            parse_cpu_list_ids("0-8191,0-8191").expect("duplicate bound list parses");
        assert_eq!(repeated_bound.len(), MAX_CPU_SET_MEMBERS);

        // Duplicates cannot be used to smuggle an over-bound list through.
        assert!(parse_cpu_list_ids("0-8191,0-8191,8192").is_none());
        assert!(parse_cpu_list_ids("0-8191,0-8191,0-8191,0-8191").is_some());
    }

    #[test]
    fn plan146_legacy_fallback_weight_is_unbounded_by_core_count() {
        // Sparse membership whose identities exceed the online/logical core
        // count must not be dropped: the fallback weight is the distinct
        // identity count, not a clamp to the core count.
        let mut mem = MemorySource::new().with_logical_cores(2);
        mem.add_file(
            "/sys/devices/system/cpu/cpufreq/policy0/affected_cpus",
            "0,2,10000\n",
        );
        mem.add_file(
            "/sys/devices/system/cpu/cpufreq/policy0/cpuinfo_cur_freq",
            "1500000\n",
        );
        let source = ProcSource::for_memory(mem);
        assert_eq!(source.cpu_frequency_hz(), Some(1_500_000_000));

        // Dense ordinary fixtures keep the pre-Plan-141 `affected_cpus`
        // weight, and the documented fallback order is unchanged.
        let mut dense = MemorySource::new().with_logical_cores(4);
        dense.add_file(
            "/sys/devices/system/cpu/cpufreq/policy0/affected_cpus",
            "0-1\n",
        );
        dense.add_file(
            "/sys/devices/system/cpu/cpufreq/policy0/cpuinfo_cur_freq",
            "2000000\n",
        );
        dense.add_file(
            "/sys/devices/system/cpu/cpufreq/policy1/affected_cpus",
            "2-3\n",
        );
        dense.add_file(
            "/sys/devices/system/cpu/cpufreq/policy1/cpuinfo_cur_freq",
            "1000000\n",
        );
        assert_eq!(
            ProcSource::for_memory(dense).cpu_frequency_hz(),
            Some(1_500_000_000)
        );

        // Malformed `affected_cpus` still falls back to `related_cpus` and
        // then to the existing bounded default weight of 1.
        let mut fallbacks = MemorySource::new().with_logical_cores(4);
        fallbacks.add_file(
            "/sys/devices/system/cpu/cpufreq/policy0/affected_cpus",
            "not a cpu list\n",
        );
        fallbacks.add_file(
            "/sys/devices/system/cpu/cpufreq/policy0/related_cpus",
            "0-2\n",
        );
        fallbacks.add_file(
            "/sys/devices/system/cpu/cpufreq/policy0/cpuinfo_cur_freq",
            "2000000\n",
        );
        let source = ProcSource::for_memory(fallbacks);
        assert_eq!(source.cpu_frequency_hz(), Some(2_000_000_000));

        let mut bounded_default = MemorySource::new().with_logical_cores(4);
        bounded_default.add_file(
            "/sys/devices/system/cpu/cpufreq/policy0/cpuinfo_cur_freq",
            "2000000\n",
        );
        bounded_default.add_file(
            "/sys/devices/system/cpu/cpufreq/policy1/cpuinfo_cur_freq",
            "1000000\n",
        );
        // Neither policy exposes membership, so each keeps the bounded
        // default weight of 1 and the average is the plain mean.
        assert_eq!(
            ProcSource::for_memory(bounded_default).cpu_frequency_hz(),
            Some(1_500_000_000)
        );
    }

    #[test]
    fn plan146_sparse_related_cpus_keeps_identity_weight_in_structural_path() {
        // Structural membership with sparse identities intersects against the
        // live online set by identity, so a CPU whose number exceeds the
        // host core count still contributes its own weight.
        let mut mem = MemorySource::new().with_logical_cores(2);
        mem.add_file("/sys/devices/system/cpu/online", "0,2,10000\n");
        mem.add_file(
            "/sys/devices/system/cpu/cpufreq/policy0/related_cpus",
            "0,2,10000\n",
        );
        mem.add_file(
            "/sys/devices/system/cpu/cpufreq/policy0/cpuinfo_cur_freq",
            "1000000\n",
        );
        mem.add_file(
            "/sys/devices/system/cpu/cpufreq/policy1/related_cpus",
            "0,2,10000\n",
        );
        mem.add_file(
            "/sys/devices/system/cpu/cpufreq/policy1/cpuinfo_cur_freq",
            "3000000\n",
        );
        let mut cache = CpuFreqStructuralCache::default();
        assert_eq!(
            ProcSource::for_memory(mem).cpu_frequency_hz_with_cache(&mut cache),
            Some(2_000_000_000)
        );
    }
}
