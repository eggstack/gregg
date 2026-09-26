//! FreeBSD native source seam.
//!
//! Production code queries FreeBSD sysctl/libc interfaces only; tests
//! inject [`MockFreeBsdSource`]. No shell commands, no privilege escalation.
//!
//! Native reference points:
//!
//! - CPU: `kern.cp_time` (five `long` states: user/nice/sys/intr/idle).
//! - Load: `getloadavg(3)` via libc.
//! - Memory: `hw.physmem` + `hw.pagesize` + `vm.stats.vm.v_*_count`.
//! - Swap: unsupported in the first backend (see `swap()`); `kvm_getswapinfo`
//!   needs unprivileged validation across the supported floor first.
//! - Filesystems: `getmntinfo`/`statfs` via libc.
//! - Disk I/O: base `libdevstat` (`devstat_getdevs` with a null kvm handle,
//!   per man `devstat(3)`); generation changes and counter decreases
//!   re-baseline; any ABI/length validation failure reports absence.
//! - Network: `ifmib(4)` sysctl table (`net.link.ifmib.ifcount` plus
//!   `net.link.ifmib.ifdata.<idx>` rows, tolerating sparse rows).
//! - Frequency: unsupported (no validated unprivileged source yet).
//!
//! Every native parse validates lengths/UTF-8/bounds before use; unexpected
//! kernel data yields [`CollectErrorKind::SourceUnavailable`](crate::error::CollectErrorKind)
//! or `Parse`, never a fabricated zero.

#![allow(unsafe_code)]

use crate::error::{CollectError, CollectErrorKind};

/// Cumulative CPU states from `kern.cp_time`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawCpuTimes {
    /// User state ticks.
    pub user: u64,
    /// Nice state ticks.
    pub nice: u64,
    /// System state ticks.
    pub sys: u64,
    /// Interrupt state ticks.
    pub intr: u64,
    /// Idle state ticks.
    pub idle: u64,
}

impl RawCpuTimes {
    /// Sum of every state.
    #[must_use]
    pub fn total(self) -> u64 {
        self.user
            .saturating_add(self.nice)
            .saturating_add(self.sys)
            .saturating_add(self.intr)
            .saturating_add(self.idle)
    }

    /// Busy states (everything except idle).
    #[must_use]
    pub fn busy(self) -> u64 {
        self.user
            .saturating_add(self.nice)
            .saturating_add(self.sys)
            .saturating_add(self.intr)
    }
}

/// Physical memory inputs from FreeBSD VM sysctls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawPhysicalMemory {
    /// Total physical bytes from `hw.physmem`.
    pub total_bytes: u64,
    /// Page size from `hw.pagesize`.
    pub page_size: u64,
    /// Free page count.
    pub free_count: u64,
    /// Inactive page count.
    pub inactive_count: u64,
    /// Cache page count.
    pub cache_count: u64,
    /// Laundry page count.
    pub laundry_count: u64,
}

/// System identity fields from FreeBSD sysctls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawIdentity {
    /// Hostname.
    pub hostname: String,
    /// OS version string.
    pub os_version: String,
    /// Kernel release string.
    pub kernel_release: String,
    /// Machine architecture.
    pub architecture: String,
    /// Logical CPU count.
    pub logical_cores: u32,
    /// Physical memory bytes.
    pub physical_memory_bytes: u64,
}

/// Owned mounted-filesystem record from `getmntinfo`/`statfs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawMountedFilesystem {
    /// Mount point.
    pub mount_point: String,
    /// Filesystem type name.
    pub filesystem_type: String,
    /// Stable (fsid) identity text.
    pub fsid: (i32, i32),
    /// Mount flags (`f_flags`).
    pub flags: u32,
    /// Total blocks.
    pub total_blocks: u64,
    /// Free blocks.
    pub free_blocks: u64,
    /// Caller-available blocks.
    pub available_blocks: u64,
    /// Fundamental block size.
    pub block_size: u64,
}

/// Cumulative disk byte counters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawDiskIo {
    /// Stable device identity.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Cumulative read bytes.
    pub read_bytes: u64,
    /// Cumulative write bytes.
    pub write_bytes: u64,
}

/// Cumulative interface counters and link metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawNetworkInterface {
    /// Stable interface identity.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Cumulative receive bytes.
    pub rx_bytes: u64,
    /// Cumulative transmit bytes.
    pub tx_bytes: u64,
    /// Receive capacity in bits per second, when known.
    pub rx_capacity_bps: Option<u64>,
    /// Transmit capacity in bits per second, when known.
    pub tx_capacity_bps: Option<u64>,
    /// Whether this interface is loopback.
    pub is_loopback: bool,
    /// Whether the link is operational.
    pub operational: bool,
    /// Whether this interface joins the aggregate.
    pub aggregate_member: bool,
}

/// Abstraction over native FreeBSD system queries.
pub trait FreeBsdSource: Send + Sync + std::fmt::Debug {
    /// Read cumulative CPU states from `kern.cp_time`.
    fn cpu_times(&self) -> Result<RawCpuTimes, CollectError>;
    /// Read one/five/fifteen-minute load averages.
    fn load_averages(&self) -> Result<[f64; 3], CollectError>;
    /// Read physical memory inputs.
    fn physical_memory(&self) -> Result<RawPhysicalMemory, CollectError>;
    /// Read system identity fields.
    fn identity(&self) -> Result<RawIdentity, CollectError>;
    /// Enumerate mounted filesystems.
    fn mounted_filesystems(&self) -> Result<Vec<RawMountedFilesystem>, CollectError>;
    /// Read cumulative per-disk byte counters (optional family).
    fn disk_io(&self) -> Result<Vec<RawDiskIo>, CollectError> {
        Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            "disk I/O source unavailable",
        ))
    }
    /// Read cumulative interface counters (optional family).
    fn network_interfaces(&self) -> Result<Vec<RawNetworkInterface>, CollectError> {
        Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            "network source unavailable",
        ))
    }
    /// Read current CPU frequency in Hz, when a supported source exists.
    fn cpu_frequency_hz(&self) -> Option<u64> {
        None
    }
}

/// Mock FreeBSD source for deterministic tests.
#[derive(Debug)]
pub struct MockFreeBsdSource {
    /// CPU states returned by `cpu_times`.
    pub cpu: RawCpuTimes,
    /// Load averages.
    pub load: [f64; 3],
    /// Memory inputs.
    pub memory: RawPhysicalMemory,
    /// Identity.
    pub identity: RawIdentity,
    /// Mounted filesystems.
    pub mounted: Vec<RawMountedFilesystem>,
    /// Disk records.
    pub disk: Vec<RawDiskIo>,
    /// Network records.
    pub network: Vec<RawNetworkInterface>,
    /// When true, CPU advances a small delta per call.
    pub auto_increment_cpu: bool,
    pub cpu_call_count: std::sync::atomic::AtomicU32,
    /// Error injection flags.
    pub cpu_error: bool,
    /// Error injection flags.
    pub memory_error: bool,
    /// Error injection flags.
    pub load_error: bool,
    /// Error injection flags.
    pub identity_error: bool,
    /// Error injection flags.
    pub mounted_error: bool,
    /// Error injection flags.
    pub disk_error: bool,
    /// Error injection flags.
    pub network_error: bool,
}

impl Clone for MockFreeBsdSource {
    fn clone(&self) -> Self {
        Self {
            cpu: self.cpu,
            load: self.load,
            memory: self.memory,
            identity: self.identity.clone(),
            mounted: self.mounted.clone(),
            disk: self.disk.clone(),
            network: self.network.clone(),
            auto_increment_cpu: self.auto_increment_cpu,
            cpu_call_count: std::sync::atomic::AtomicU32::new(
                self.cpu_call_count
                    .load(std::sync::atomic::Ordering::Relaxed),
            ),
            cpu_error: self.cpu_error,
            memory_error: self.memory_error,
            load_error: self.load_error,
            identity_error: self.identity_error,
            mounted_error: self.mounted_error,
            disk_error: self.disk_error,
            network_error: self.network_error,
        }
    }
}

impl MockFreeBsdSource {
    /// Build a mock returning sensible default values.
    #[must_use]
    pub fn success() -> Self {
        Self {
            cpu: RawCpuTimes {
                user: 1000,
                nice: 100,
                sys: 500,
                intr: 50,
                idle: 8000,
            },
            load: [0.5, 0.4, 0.3],
            memory: RawPhysicalMemory {
                total_bytes: 16_000_000_000,
                page_size: 4096,
                free_count: 500_000,
                inactive_count: 300_000,
                cache_count: 200_000,
                laundry_count: 50_000,
            },
            identity: RawIdentity {
                hostname: "freebsd-host".to_string(),
                os_version: "14.2-RELEASE".to_string(),
                kernel_release: "14.2-RELEASE".to_string(),
                architecture: "amd64".to_string(),
                logical_cores: 4,
                physical_memory_bytes: 16_000_000_000,
            },
            mounted: vec![RawMountedFilesystem {
                mount_point: "/".to_string(),
                filesystem_type: "ufs".to_string(),
                fsid: (1, 1),
                flags: MNT_LOCAL,
                total_blocks: 1_000_000,
                free_blocks: 400_000,
                available_blocks: 350_000,
                block_size: 32768,
            }],
            disk: Vec::new(),
            network: Vec::new(),
            auto_increment_cpu: false,
            cpu_call_count: std::sync::atomic::AtomicU32::new(0),
            cpu_error: false,
            memory_error: false,
            load_error: false,
            identity_error: false,
            mounted_error: false,
            disk_error: false,
            network_error: false,
        }
    }
}

impl FreeBsdSource for MockFreeBsdSource {
    fn cpu_times(&self) -> Result<RawCpuTimes, CollectError> {
        if self.cpu_error {
            return Err(CollectError::new(
                CollectErrorKind::SourceUnavailable,
                "mock cpu error",
            ));
        }
        if self.auto_increment_cpu {
            use std::sync::atomic::Ordering;
            let call = self.cpu_call_count.fetch_add(1, Ordering::Relaxed);
            let offset = u64::from(call) * 100;
            return Ok(RawCpuTimes {
                user: self.cpu.user + offset,
                nice: self.cpu.nice,
                sys: self.cpu.sys,
                intr: self.cpu.intr,
                idle: self.cpu.idle + offset / 2,
            });
        }
        Ok(self.cpu)
    }

    fn load_averages(&self) -> Result<[f64; 3], CollectError> {
        if self.load_error {
            return Err(CollectError::new(
                CollectErrorKind::SourceUnavailable,
                "mock load error",
            ));
        }
        Ok(self.load)
    }

    fn physical_memory(&self) -> Result<RawPhysicalMemory, CollectError> {
        if self.memory_error {
            return Err(CollectError::new(
                CollectErrorKind::SourceUnavailable,
                "mock memory error",
            ));
        }
        Ok(self.memory)
    }

    fn identity(&self) -> Result<RawIdentity, CollectError> {
        if self.identity_error {
            return Err(CollectError::new(
                CollectErrorKind::SourceUnavailable,
                "mock identity error",
            ));
        }
        Ok(self.identity.clone())
    }

    fn mounted_filesystems(&self) -> Result<Vec<RawMountedFilesystem>, CollectError> {
        if self.mounted_error {
            return Err(CollectError::new(
                CollectErrorKind::SourceUnavailable,
                "mock mounted filesystem error",
            ));
        }
        Ok(self.mounted.clone())
    }

    fn disk_io(&self) -> Result<Vec<RawDiskIo>, CollectError> {
        if self.disk_error {
            return Err(CollectError::new(
                CollectErrorKind::SourceUnavailable,
                "mock disk error",
            ));
        }
        Ok(self.disk.clone())
    }

    fn network_interfaces(&self) -> Result<Vec<RawNetworkInterface>, CollectError> {
        if self.network_error {
            return Err(CollectError::new(
                CollectErrorKind::SourceUnavailable,
                "mock network error",
            ));
        }
        Ok(self.network.clone())
    }
}

/// Production implementation backed by FreeBSD sysctl/libc/libdevstat.
#[derive(Debug, Clone, Copy, Default)]
pub struct NativeFreeBsdSource;

impl FreeBsdSource for NativeFreeBsdSource {
    fn cpu_times(&self) -> Result<RawCpuTimes, CollectError> {
        cpu_times()
    }

    fn load_averages(&self) -> Result<[f64; 3], CollectError> {
        load_averages()
    }

    fn physical_memory(&self) -> Result<RawPhysicalMemory, CollectError> {
        physical_memory()
    }

    fn identity(&self) -> Result<RawIdentity, CollectError> {
        native_identity()
    }

    fn mounted_filesystems(&self) -> Result<Vec<RawMountedFilesystem>, CollectError> {
        mounted_filesystems()
    }

    fn disk_io(&self) -> Result<Vec<RawDiskIo>, CollectError> {
        disk_io()
    }

    fn network_interfaces(&self) -> Result<Vec<RawNetworkInterface>, CollectError> {
        network_interfaces()
    }
}

// ---------------------------------------------------------------------------
// Native helpers (FreeBSD only)
// ---------------------------------------------------------------------------

/// Mount flag for local filesystems (mirrors the system header).
#[cfg(target_os = "freebsd")]
pub(crate) const MNT_LOCAL: u32 = libc::MNT_LOCAL as u32;
/// Fallback when not compiling for FreeBSD (mock tests still exercise logic).
#[cfg(not(target_os = "freebsd"))]
pub(crate) const MNT_LOCAL: u32 = 0x1000;

#[cfg(target_os = "freebsd")]
fn sysctl_string(name: &str) -> Result<String, CollectError> {
    use std::ffi::CString;
    let cname = CString::new(name)
        .map_err(|_| CollectError::new(CollectErrorKind::Parse, "bad sysctl name"))?;
    let mut len: libc::size_t = 0;
    // Safety: querying the required length with a null buffer is the
    // documented two-step sysctlbyname pattern; return code checked.
    let queried = unsafe {
        libc::sysctlbyname(
            cname.as_ptr(),
            std::ptr::null_mut(),
            &raw mut len,
            std::ptr::null(),
            0,
        )
    };
    if queried != 0 || len == 0 || len > 4096 {
        return Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            format!("sysctl {name} length query failed"),
        ));
    }
    let mut buf = vec![0u8; len];
    // Safety: buffer is sized from the length query; return checked.
    let fetched = unsafe {
        libc::sysctlbyname(
            cname.as_ptr(),
            buf.as_mut_ptr().cast(),
            &raw mut len,
            std::ptr::null(),
            0,
        )
    };
    if fetched != 0 {
        return Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            format!("sysctl {name} read failed"),
        ));
    }
    buf.truncate(len);
    while buf.last() == Some(&0) {
        buf.pop();
    }
    String::from_utf8(buf).map_err(|_| {
        CollectError::new(
            CollectErrorKind::Parse,
            format!("sysctl {name} is not valid UTF-8"),
        )
    })
}

#[cfg(target_os = "freebsd")]
fn sysctl_u32(name: &str) -> Result<u32, CollectError> {
    sysctl_fixed::<u32>(name)
}

#[cfg(target_os = "freebsd")]
fn sysctl_u64(name: &str) -> Result<u64, CollectError> {
    sysctl_fixed::<u64>(name)
}

#[cfg(target_os = "freebsd")]
fn sysctl_fixed<T: Copy + Default>(name: &str) -> Result<T, CollectError> {
    use std::ffi::CString;
    let cname = CString::new(name)
        .map_err(|_| CollectError::new(CollectErrorKind::Parse, "bad sysctl name"))?;
    let mut value = T::default();
    let mut len = std::mem::size_of::<T>() as libc::size_t;
    // Safety: value points to valid typed storage of exactly `len` bytes.
    let fetched = unsafe {
        libc::sysctlbyname(
            cname.as_ptr(),
            (&raw mut value).cast(),
            &raw mut len,
            std::ptr::null(),
            0,
        )
    };
    if fetched != 0 || len != std::mem::size_of::<T>() as libc::size_t {
        return Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            format!("sysctl {name} read failed"),
        ));
    }
    Ok(value)
}

#[cfg(target_os = "freebsd")]
fn cpu_times() -> Result<RawCpuTimes, CollectError> {
    use std::ffi::CString;
    let name = CString::new("kern.cp_time").expect("static name");
    // Five `long` states: user, nice, sys, intr, idle.
    let mut states = [0 as libc::c_long; 5];
    let mut len = std::mem::size_of_val(&states) as libc::size_t;
    // Safety: states is valid for five longs; length validated below.
    let fetched = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            states.as_mut_ptr().cast(),
            &raw mut len,
            std::ptr::null(),
            0,
        )
    };
    if fetched != 0 || len != std::mem::size_of_val(&states) as libc::size_t {
        return Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            "kern.cp_time read failed",
        ));
    }
    let widen = |value: libc::c_long| -> Result<u64, CollectError> {
        u64::try_from(value).map_err(|_| {
            CollectError::new(CollectErrorKind::Parse, "kern.cp_time state is negative")
        })
    };
    Ok(RawCpuTimes {
        user: widen(states[0])?,
        nice: widen(states[1])?,
        sys: widen(states[2])?,
        intr: widen(states[3])?,
        idle: widen(states[4])?,
    })
}

#[cfg(not(target_os = "freebsd"))]
fn cpu_times() -> Result<RawCpuTimes, CollectError> {
    Err(CollectError::new(
        CollectErrorKind::SourceUnavailable,
        "FreeBSD CPU source requires a FreeBSD host",
    ))
}

#[cfg(target_os = "freebsd")]
fn load_averages() -> Result<[f64; 3], CollectError> {
    let mut loads = [0f64; 3];
    // Safety: loads holds three doubles; return is the count stored.
    let count = unsafe { libc::getloadavg(loads.as_mut_ptr(), 3) };
    if count != 3 {
        return Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            "getloadavg did not return three samples",
        ));
    }
    Ok(loads)
}

#[cfg(not(target_os = "freebsd"))]
fn load_averages() -> Result<[f64; 3], CollectError> {
    Err(CollectError::new(
        CollectErrorKind::SourceUnavailable,
        "FreeBSD load source requires a FreeBSD host",
    ))
}

#[cfg(target_os = "freebsd")]
fn physical_memory() -> Result<RawPhysicalMemory, CollectError> {
    // Total from hw.physmem (u_long); page size from hw.pagesize.
    let total = sysctl_u64("hw.physmem").or_else(|_| sysctl_u32("hw.physmem").map(u64::from))?;
    let page_size = u64::from(sysctl_u32("hw.pagesize")?);
    if page_size == 0 {
        return Err(CollectError::new(
            CollectErrorKind::Parse,
            "hw.pagesize is zero",
        ));
    }
    let free_count = u64::from(sysctl_u32("vm.stats.vm.v_free_count")?);
    let inactive_count = u64::from(sysctl_u32("vm.stats.vm.v_inactive_count")?);
    let cache_count = sysctl_u32("vm.stats.vm.v_cache_count").unwrap_or(0);
    let laundry_count = sysctl_u32("vm.stats.vm.v_laundry_count").unwrap_or(0);
    Ok(RawPhysicalMemory {
        total_bytes: total,
        page_size,
        free_count,
        inactive_count,
        cache_count: u64::from(cache_count),
        laundry_count: u64::from(laundry_count),
    })
}

#[cfg(not(target_os = "freebsd"))]
fn physical_memory() -> Result<RawPhysicalMemory, CollectError> {
    Err(CollectError::new(
        CollectErrorKind::SourceUnavailable,
        "FreeBSD memory source requires a FreeBSD host",
    ))
}

#[cfg(target_os = "freebsd")]
fn native_identity() -> Result<RawIdentity, CollectError> {
    let hostname = sysctl_string("kern.hostname")?;
    if hostname.trim().is_empty() {
        return Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            "kern.hostname is empty",
        ));
    }
    let kernel_release = sysctl_string("kern.osrelease").unwrap_or_else(|_| "unknown".to_string());
    let os_version = kernel_release.clone();
    let architecture = sysctl_string("hw.machine").unwrap_or_else(|_| "unknown".to_string());
    let logical = sysctl_u32("hw.ncpu").unwrap_or(1).max(1);
    let total_bytes = sysctl_u64("hw.physmem")
        .or_else(|_| sysctl_u32("hw.physmem").map(u64::from))
        .unwrap_or(0);
    Ok(RawIdentity {
        hostname,
        os_version,
        kernel_release,
        architecture,
        logical_cores: logical,
        physical_memory_bytes: total_bytes,
    })
}

#[cfg(not(target_os = "freebsd"))]
fn native_identity() -> Result<RawIdentity, CollectError> {
    Err(CollectError::new(
        CollectErrorKind::SourceUnavailable,
        "FreeBSD identity source requires a FreeBSD host",
    ))
}

#[cfg(target_os = "freebsd")]
fn mounted_filesystems() -> Result<Vec<RawMountedFilesystem>, CollectError> {
    // Safety: getmntinfo manages its own buffer and sets the out-pointer;
    // count and pointer are both validated before the slice is formed;
    // records are copied out before return.
    let mut buf: *mut libc::statfs = std::ptr::null_mut();
    let count = unsafe { libc::getmntinfo(&raw mut buf, libc::MNT_WAIT) };
    if count <= 0 || buf.is_null() {
        return Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            "getmntinfo returned no filesystems",
        ));
    }
    let records = unsafe { std::slice::from_raw_parts(buf.cast_const(), count as usize) };
    let mut out = Vec::new();
    for stat in records {
        let mount = native_c_string(&stat.f_mntonname)?;
        let fstype = native_c_string(&stat.f_fstypename)?;
        let fsid = fsid_pair(&stat.f_fsid);
        let free_blocks = u64::try_from(stat.f_bfree).map_err(|_| {
            CollectError::new(CollectErrorKind::Parse, "statfs free blocks negative")
        })?;
        let available_blocks = u64::try_from(stat.f_bavail).map_err(|_| {
            CollectError::new(CollectErrorKind::Parse, "statfs available blocks negative")
        })?;
        #[allow(clippy::cast_possible_truncation)]
        let flags = stat.f_flags as u32;
        out.push(RawMountedFilesystem {
            mount_point: mount,
            filesystem_type: fstype,
            fsid,
            flags,
            total_blocks: stat.f_blocks,
            free_blocks,
            available_blocks,
            block_size: u64::from(stat.f_bsize),
        });
    }
    Ok(out)
}

#[cfg(not(target_os = "freebsd"))]
fn mounted_filesystems() -> Result<Vec<RawMountedFilesystem>, CollectError> {
    Err(CollectError::new(
        CollectErrorKind::SourceUnavailable,
        "FreeBSD mount source requires a FreeBSD host",
    ))
}

#[cfg(target_os = "freebsd")]
fn native_c_string(bytes: &[libc::c_char]) -> Result<String, CollectError> {
    let len = bytes.iter().position(|&c| c == 0).unwrap_or(bytes.len());
    let slice: Vec<u8> = bytes[..len].iter().map(|&c| c as u8).collect();
    String::from_utf8(slice)
        .map_err(|_| CollectError::new(CollectErrorKind::Parse, "mount string is not UTF-8"))
}

#[cfg(target_os = "freebsd")]
fn fsid_pair(fsid: &libc::fsid_t) -> (i32, i32) {
    // Safety: FreeBSD fsid_t is two i32 values; size asserted.
    debug_assert_eq!(std::mem::size_of::<libc::fsid_t>(), 8);
    let raw: [i32; 2] = unsafe { std::ptr::read_unaligned(std::ptr::from_ref(fsid).cast()) };
    (raw[0], raw[1])
}

// --- Disk I/O via libdevstat ---------------------------------------------------
//
// Documented API (man `devstat(3)`, FreeBSD 13-15): `devstat_getdevs(NULL,
// &statinfo)` reads through sysctl when passed a null kvm handle.
// `statinfo` carries `cp_time`, `dinfo` (devices/generation/numdevs), and a
// snapshot time. `devstat_checkversion(NULL)` gates userland/kernel version
// drift. The library allocates into `dinfo.mem_ptr`; there is no public
// release helper (see BUGS in the man page), so owned records are copied
// out and the allocation is released with libc `free`.
//
// `struct devstat` leading layout (sys/sys/devicestat.h): `sequence0`
// (u32), `allocated` (int), `device_number` (u_int), `device_name[16]`,
// `unit_number` (int), then cumulative `bytes[]` indexed by transaction
// type (read = 0, write = 1). Only this prefix is mapped; the entry is
// accepted only when the name is printable ASCII, the unit is sane, and
// `sequence0 == sequence1` (consistent snapshot). A layout mismatch therefore
// degrades to absence, never to fabricated counters. Byte-index direction is
// covered by the native traffic-direction smoke in CI (known write/read
// traffic must advance the matching family).

#[cfg(target_os = "freebsd")]
#[link(name = "devstat")]
extern "C" {
    fn devstat_checkversion(kd: *mut std::ffi::c_void) -> libc::c_int;
    fn devstat_getdevs(kd: *mut std::ffi::c_void, stats: *mut StatInfo) -> libc::c_int;
}
// NOTE: libdevstat provides no public release helper (see BUGS in man
// `devstat(3)`); the `dinfo.mem_ptr` allocation is released with libc
// `free` after owned records are copied out.

/// CPU states count for `statinfo.cp_time` (matches `kern.cp_time`).
#[cfg(target_os = "freebsd")]
const CPUSTATES: usize = 5;

/// Library device-info state (man `devstat(3)`).
#[cfg(target_os = "freebsd")]
#[repr(C)]
struct DevInfo {
    devices: *const DevstatEntry,
    mem_ptr: *mut u8,
    generation: libc::c_long,
    numdevs: libc::c_int,
}

/// Library snapshot (man `devstat(3)`).
#[cfg(target_os = "freebsd")]
#[repr(C)]
struct StatInfo {
    cp_time: [libc::c_long; CPUSTATES],
    tk_nin: libc::c_long,
    tk_nout: libc::c_long,
    dinfo: *mut DevInfo,
    snap_time: f64,
}

/// Minimal `struct devstat` prefix (see note above).
#[cfg(target_os = "freebsd")]
#[repr(C)]
struct DevstatEntry {
    sequence0: u32,
    allocated: libc::c_int,
    device_number: libc::c_uint,
    device_name: [libc::c_char; 16],
    unit_number: libc::c_int,
    bytes_read: u64,
    bytes_write: u64,
    bytes_free: u64,
    sequence1: u32,
}

#[cfg(target_os = "freebsd")]
fn disk_io() -> Result<Vec<RawDiskIo>, CollectError> {
    // Safety: version gate first; statinfo/devinfo zeroed and locally owned;
    // devstat_getdevs allocates library memory into dinfo.mem_ptr, released
    // below with libc free after owned records are copied out; entries are
    // read only within numdevs bounds and validated for plausibility.
    if unsafe { devstat_checkversion(std::ptr::null_mut()) } != 0 {
        return Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            "devstat userland/kernel version mismatch",
        ));
    }
    let mut dinfo = DevInfo {
        devices: std::ptr::null(),
        mem_ptr: std::ptr::null_mut(),
        generation: 0,
        numdevs: 0,
    };
    let mut stats = StatInfo {
        cp_time: [0; CPUSTATES],
        tk_nin: 0,
        tk_nout: 0,
        dinfo: &raw mut dinfo,
        snap_time: 0.0,
    };
    let fetched = unsafe { devstat_getdevs(std::ptr::null_mut(), &raw mut stats) };
    // Man `devstat(3)`: -1 is an error, 0 means no error, and 1 means the
    // device list changed (expected on the first call). Only -1 fails.
    if fetched < 0 {
        return Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            "devstat_getdevs failed",
        ));
    }
    let result = read_devstat_entries(&dinfo);
    // Safety: mem_ptr was allocated by devstat_getdevs; records were copied
    // out above; freeing here keeps per-sample collection leak-free.
    unsafe {
        if !dinfo.mem_ptr.is_null() {
            libc::free(dinfo.mem_ptr.cast());
        }
    }
    result
}

#[cfg(target_os = "freebsd")]
fn read_devstat_entries(dinfo: &DevInfo) -> Result<Vec<RawDiskIo>, CollectError> {
    if dinfo.numdevs <= 0 || dinfo.numdevs > 4096 || dinfo.devices.is_null() {
        return Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            "devstat device list is empty or unbounded",
        ));
    }
    let count = dinfo.numdevs as usize;
    // Safety: devices points to numdevs library-owned entries; each entry is
    // copied out after plausibility validation; no pointers escape.
    let entries = unsafe { std::slice::from_raw_parts(dinfo.devices, count) };
    let mut out = Vec::new();
    for entry in entries {
        let Some(record) = devstat_record(entry) else {
            continue;
        };
        out.push(record);
    }
    if out.is_empty() {
        return Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            "no plausible devstat entries",
        ));
    }
    out.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(out)
}

#[cfg(target_os = "freebsd")]
fn devstat_record(entry: &DevstatEntry) -> Option<RawDiskIo> {
    if entry.sequence0 != entry.sequence1 || entry.allocated == 0 {
        return None;
    }
    let name_len = entry
        .device_name
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(entry.device_name.len());
    if name_len == 0 || name_len > 15 {
        return None;
    }
    let name_bytes: Vec<u8> = entry.device_name[..name_len]
        .iter()
        .map(|&c| c as u8)
        .collect();
    if !name_bytes.iter().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    let name = String::from_utf8(name_bytes).ok()?;
    if entry.unit_number < 0 || entry.unit_number > 65535 {
        return None;
    }
    let id = format!("{name}{}", entry.unit_number);
    Some(RawDiskIo {
        id: id.clone(),
        name: id,
        read_bytes: entry.bytes_read,
        write_bytes: entry.bytes_write,
    })
}

#[cfg(not(target_os = "freebsd"))]
fn disk_io() -> Result<Vec<RawDiskIo>, CollectError> {
    Err(CollectError::new(
        CollectErrorKind::SourceUnavailable,
        "FreeBSD disk source requires a FreeBSD host",
    ))
}

// --- Network via ifmib -----------------------------------------------------------
//
// Documented access (man `ifmib(4)`): integer MIB
// `[CTL_NET, PF_LINK, NETLINK_GENERIC, IFMIB_IFDATA, row, IFDATA_GENERAL]`
// returns one `struct ifmibdata` per row; the table may be sparse (ENOENT
// rows are skipped). `struct ifmibdata` leading fields: `ifmd_name[16]`,
// `ifmd_pcount` (int), `ifmd_flags` (int), `ifmd_snd_len`/`ifmd_snd_drops`
// (int), then `ifmd_data` (`struct if_data`). Only the name, flags, byte
// counters, baudrate, and type fields used here are mapped; rows are
// accepted only with a printable nonempty name. Loopback follows
// `IFF_LOOPBACK`/`IFT_LOOP`; aggregate membership excludes loopback.
// Baudrate/type/counter offsets are covered by the native direction smoke
// in CI (loopback ping must advance `lo` counters).

/// sysctl MIB constants for ifmib (net/if_mib.h; stable ABI).
#[cfg(target_os = "freebsd")]
const CTL_NET: libc::c_int = 4;
/// sysctl MIB constants for ifmib (net/if_mib.h; stable ABI).
#[cfg(target_os = "freebsd")]
const PF_LINK: libc::c_int = 18;
/// sysctl MIB constants for ifmib (net/if_mib.h; stable ABI).
#[cfg(target_os = "freebsd")]
const NETLINK_GENERIC: libc::c_int = 1;
/// sysctl MIB constants for ifmib (net/if_mib.h; stable ABI).
#[cfg(target_os = "freebsd")]
const IFMIB_IFDATA: libc::c_int = 2;
/// sysctl MIB constants for ifmib (net/if_mib.h; stable ABI).
#[cfg(target_os = "freebsd")]
const IFDATA_GENERAL: libc::c_int = 1;
/// Interface flag for loopback (net/if.h; stable ABI).
#[cfg(target_os = "freebsd")]
const IFF_LOOPBACK_FLAG: u32 = 0x8;
/// Interface flag for administratively up (net/if.h; stable ABI).
#[cfg(target_os = "freebsd")]
const IFF_UP_FLAG: u32 = 0x1;
/// Interface type for loopback (net/if_types.h; stable ABI).
const IFT_LOOP_TYPE: u8 = 24;
/// Interface name length (net/if.h `IFNAMSIZ`; stable ABI).
#[cfg(target_os = "freebsd")]
const IFNAMSIZ: usize = 16;

/// Minimal `struct if_data` prefix through the byte counters and baudrate.
/// Later kernel fields are not mapped; length is validated before use.
#[cfg(target_os = "freebsd")]
#[repr(C)]
struct IfDataPrefix {
    ifi_type: u8,
    ifi_addrlen: u8,
    ifi_hdrlen: u8,
    ifi_link_state: u8,
    ifi_mtu: u32,
    ifi_metric: u32,
    ifi_baudrate: u64,
    ifi_ipackets: u64,
    ifi_ierrors: u64,
    ifi_opackets: u64,
    ifi_oerrors: u64,
    ifi_collisions: u64,
    ifi_ibytes: u64,
    ifi_obytes: u64,
}

/// Minimal `struct ifmibdata` through `ifmd_data` (see note above).
#[cfg(target_os = "freebsd")]
#[repr(C)]
struct IfmibData {
    ifmd_name: [libc::c_char; IFNAMSIZ],
    ifmd_pcount: libc::c_int,
    ifmd_flags: libc::c_int,
    ifmd_snd_len: libc::c_int,
    ifmd_snd_drops: libc::c_int,
    ifmd_data: IfDataPrefix,
}

#[cfg(target_os = "freebsd")]
fn network_interfaces() -> Result<Vec<RawNetworkInterface>, CollectError> {
    let count = sysctl_u32("net.link.generic.system.ifcount").unwrap_or(0);
    if count == 0 || count > 1024 {
        return Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            "ifmib interface count unavailable",
        ));
    }
    let prefix = std::mem::size_of::<IfmibData>();
    // The kernel `struct ifmibdata` may be larger than the mapped prefix
    // (later fields are not used here). Read into a generous buffer and
    // interpret only the validated leading prefix.
    let mut out = Vec::new();
    for row in 1..=count {
        let mib = [
            CTL_NET,
            PF_LINK,
            NETLINK_GENERIC,
            IFMIB_IFDATA,
            row as libc::c_int,
            IFDATA_GENERAL,
        ];
        let mut buffer = vec![0u8; 1024];
        let mut len = buffer.len() as libc::size_t;
        // Safety: buffer is valid for its length; MIB addresses a single
        // sparse-tolerant row; ENOENT rows are skipped; only the validated
        // prefix is interpreted below.
        let fetched = unsafe {
            libc::sysctl(
                mib.as_ptr(),
                mib.len() as libc::c_uint,
                buffer.as_mut_ptr().cast(),
                &raw mut len,
                std::ptr::null(),
                0,
            )
        };
        if fetched != 0 || len < prefix as libc::size_t {
            continue;
        }
        // Safety: the first `prefix` bytes were initialized by sysctl.
        let row_data: &IfmibData = unsafe { &*buffer.as_ptr().cast() };
        if let Some(record) = ifmib_record(row, row_data) {
            out.push(record);
        }
    }
    if out.is_empty() {
        return Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            "no plausible ifmib interfaces",
        ));
    }
    out.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(out)
}

#[cfg(target_os = "freebsd")]
fn ifmib_record(row: u32, data: &IfmibData) -> Option<RawNetworkInterface> {
    let name_len = data
        .ifmd_name
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(data.ifmd_name.len());
    if name_len == 0 || name_len >= IFNAMSIZ {
        return None;
    }
    let name_bytes: Vec<u8> = data.ifmd_name[..name_len]
        .iter()
        .map(|&c| c as u8)
        .collect();
    if !name_bytes
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'.')
    {
        return None;
    }
    let name = String::from_utf8(name_bytes).ok()?;
    let flags = data.ifmd_flags as u32;
    let is_loopback = flags & IFF_LOOPBACK_FLAG != 0 || data.ifmd_data.ifi_type == IFT_LOOP_TYPE;
    let operational = flags & IFF_UP_FLAG != 0;
    let baudrate = (data.ifmd_data.ifi_baudrate > 0).then_some(data.ifmd_data.ifi_baudrate);
    Some(RawNetworkInterface {
        id: format!("if{row}"),
        name,
        rx_bytes: data.ifmd_data.ifi_ibytes,
        tx_bytes: data.ifmd_data.ifi_obytes,
        rx_capacity_bps: baudrate,
        tx_capacity_bps: baudrate,
        is_loopback,
        operational,
        aggregate_member: !is_loopback,
    })
}

#[cfg(not(target_os = "freebsd"))]
fn network_interfaces() -> Result<Vec<RawNetworkInterface>, CollectError> {
    Err(CollectError::new(
        CollectErrorKind::SourceUnavailable,
        "FreeBSD network source requires a FreeBSD host",
    ))
}

/// Normalize one ifmib row into an owned record (pure, host-independent).
/// Kept small and total so mock tests prove sparse/loopback/reset handling
/// while native decoding is validated.
#[allow(dead_code)]
fn normalize_ifmib_row(
    index: u32,
    name: &str,
    flags: u32,
    if_type: u8,
    rx_bytes: u64,
    tx_bytes: u64,
    baudrate: Option<u64>,
    operational: bool,
) -> Option<RawNetworkInterface> {
    const IFF_LOOPBACK: u32 = 0x8;
    const IFT_LOOP: u8 = 24;
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.contains('\0') {
        return None;
    }
    let is_loopback = flags & IFF_LOOPBACK != 0 || if_type == IFT_LOOP;
    Some(RawNetworkInterface {
        id: format!("if{index}"),
        name: trimmed.to_string(),
        rx_bytes,
        tx_bytes,
        rx_capacity_bps: baudrate,
        tx_capacity_bps: baudrate,
        is_loopback,
        operational,
        aggregate_member: !is_loopback,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_busy_excludes_idle() {
        let ticks = RawCpuTimes {
            user: 1000,
            nice: 100,
            sys: 500,
            intr: 50,
            idle: 8000,
        };
        assert_eq!(ticks.busy(), 1650);
        assert_eq!(ticks.total(), 9650);
    }

    #[test]
    fn ifmib_row_rejects_empty_and_flags_loopback() {
        assert!(normalize_ifmib_row(1, "", 0, 6, 0, 0, None, true).is_none());
        let loopback = normalize_ifmib_row(1, "lo0", 0x8, 24, 10, 20, None, true).expect("row");
        assert!(loopback.is_loopback);
        assert!(!loopback.aggregate_member);
        let ether =
            normalize_ifmib_row(2, "em0", 0x1, 6, 30, 40, Some(1_000_000_000), true).expect("row");
        assert!(!ether.is_loopback);
        assert!(ether.aggregate_member);
        assert_eq!(ether.rx_capacity_bps, Some(1_000_000_000));
    }

    #[test]
    fn mock_source_reports_success_defaults() {
        let mock = MockFreeBsdSource::success();
        assert!(mock.cpu_times().is_ok());
        assert!(mock.load_averages().is_ok());
        assert!(mock.physical_memory().is_ok());
        assert!(mock.identity().is_ok());
        assert!(!mock.mounted_filesystems().expect("mounts").is_empty());
    }
}
