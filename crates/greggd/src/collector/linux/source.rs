//! In-process source abstraction for procfs reads.
//!
//! Production code reads from `/proc/stat`, `/proc/loadavg`, `/proc/meminfo`,
//! `/proc/sys/kernel/{osrelease,hostname}`, `/sys/devices/system/cpu`, and
//! `/etc/os-release` through the [`ProcSource::production`] constructor.
//! Tests construct a [`ProcSource`] with explicit file contents so they can
//! exercise edge cases without depending on the host `/proc` filesystem.
//!
//! No external commands are invoked for metrics collection.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::collector::error::{CollectError, CollectErrorKind};

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
        let root = Path::new("/sys/devices/system/cpu/cpufreq");
        let policies = self.inner.read_dir(root).ok()?;
        let logical_cores = self.logical_core_count().unwrap_or(1);
        let mut weighted_sum = 0u128;
        let mut weight_sum = 0u128;
        for policy in policies {
            if !policy
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("policy"))
            {
                continue;
            }
            let affected = self.inner.read_to_string(&policy.join("affected_cpus"));
            let weight = match affected {
                Ok(raw) => parse_cpu_list(&raw, logical_cores).or_else(|| {
                    self.inner
                        .read_to_string(&policy.join("related_cpus"))
                        .ok()
                        .and_then(|raw| parse_cpu_list(&raw, logical_cores))
                }),
                Err(_) => self
                    .inner
                    .read_to_string(&policy.join("related_cpus"))
                    .ok()
                    .and_then(|raw| parse_cpu_list(&raw, logical_cores))
                    .or(Some(1)),
            }
            .unwrap_or(0);
            if weight == 0 {
                continue;
            }
            let khz = self
                .inner
                .read_to_string(&policy.join("cpuinfo_cur_freq"))
                .ok()
                .and_then(|raw| parse_positive_u64(&raw))
                .or_else(|| {
                    self.inner
                        .read_to_string(&policy.join("scaling_cur_freq"))
                        .ok()
                        .and_then(|raw| parse_positive_u64(&raw))
                });
            let Some(khz) = khz else { continue };
            let Some(hz) = khz.checked_mul(1_000) else {
                continue;
            };
            weighted_sum = weighted_sum.checked_add(u128::from(hz) * weight as u128)?;
            weight_sum = weight_sum.checked_add(weight as u128)?;
        }
        u64::try_from(weighted_sum.checked_div(weight_sum)?).ok()
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
            if self.inner.path_exists(&device.join("slaves"))
                && self
                    .inner
                    .read_dir(&device.join("slaves"))
                    .is_ok_and(|slaves| !slaves.is_empty())
            {
                continue;
            }
            let Ok(raw) = self.inner.read_to_string(&device.join("stat")) else {
                continue;
            };
            let fields: Vec<_> = raw.split_whitespace().collect();
            if fields.len() < 7 {
                continue;
            }
            let Ok(read_sectors) = fields[2].parse::<u64>() else {
                continue;
            };
            let Ok(write_sectors) = fields[6].parse::<u64>() else {
                continue;
            };
            records.push(RawDiskIo {
                id: name.to_owned(),
                name: name.to_owned(),
                read_bytes: read_sectors.checked_mul(512).ok_or_else(|| {
                    CollectError::new(
                        CollectErrorKind::Numeric,
                        "disk read-sector byte conversion overflowed",
                    )
                })?,
                write_bytes: write_sectors.checked_mul(512).ok_or_else(|| {
                    CollectError::new(
                        CollectErrorKind::Numeric,
                        "disk write-sector byte conversion overflowed",
                    )
                })?,
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
            let fields: Vec<_> = values.split_whitespace().collect();
            if fields.len() < 9 {
                continue;
            }
            let (Ok(rx_bytes), Ok(tx_bytes)) = (fields[0].parse(), fields[8].parse()) else {
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
}

impl FileSource for MemorySource {
    fn read_to_string(&self, path: &Path) -> Result<String, CollectError> {
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

fn parse_cpu_list(raw: &str, logical_cores: usize) -> Option<usize> {
    let mut count = 0usize;
    for item in raw
        .trim()
        .split(|character: char| character == ',' || character.is_whitespace())
        .filter(|item| !item.is_empty())
    {
        let (start, end) = item.split_once('-').map_or_else(
            || item.parse::<usize>().ok().map(|value| (value, value)),
            |(start, end)| Some((start.parse().ok()?, end.parse().ok()?)),
        )?;
        if end < start {
            return None;
        }
        let bounded_end = end.min(logical_cores.saturating_sub(1));
        if start <= bounded_end {
            count = count.checked_add(bounded_end - start + 1)?;
        }
    }
    (count > 0).then_some(count)
}

/// Output of [`ProcSource::read_proc_stat`].
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ParsedProcStat {
    /// Aggregate `cpu` line counters, if present. `None` if `/proc/stat` is
    /// missing the canonical `cpu` row.
    pub aggregate: Option<crate::collector::linux::CpuCounters>,
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
use crate::collector::linux::{cpu, memory};

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
}
