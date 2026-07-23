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

    /// Return the kernel-reported logical core count, if known.
    fn available_parallelism(&self) -> Option<usize>;

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

impl FileSource for HostSource {
    fn read_to_string(&self, path: &Path) -> Result<String, CollectError> {
        match fs::read_to_string(path) {
            Ok(s) => Ok(s),
            Err(err) => Err(map_io_error(path, err)),
        }
    }

    fn available_parallelism(&self) -> Option<usize> {
        std::thread::available_parallelism()
            .ok()
            .map(std::num::NonZeroUsize::get)
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

    /// Set the logical core count returned by [`Self::available_parallelism`].
    #[must_use]
    pub fn with_logical_cores(mut self, cores: usize) -> Self {
        self.logical_cores = Some(cores);
        self
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

    fn available_parallelism(&self) -> Option<usize> {
        self.logical_cores
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any>
    where
        Self: 'static,
    {
        Some(self)
    }
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
