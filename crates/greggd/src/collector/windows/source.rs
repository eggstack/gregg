//! Windows source abstraction for testable metric collection.
//!
//! Production code calls native Windows APIs behind a narrow FFI module.
//! Tests inject [`MockWindowsSource`] to exercise edge cases without
//! depending on the host state.

#![allow(unsafe_code)]

use crate::collector::error::{CollectError, CollectErrorKind};

/// Cumulative CPU time counters from `GetSystemTimes`.
///
/// Windows kernel time includes idle time. To compute busy time:
///
/// ```text
/// kernel_busy = kernel - idle
/// total = kernel + user
/// busy = total - idle = kernel_busy + user
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawCpuTimes {
    pub idle: u64,
    pub kernel: u64,
    pub user: u64,
}

impl RawCpuTimes {
    /// Sum of kernel + user + idle.
    #[must_use]
    pub fn total(self) -> u64 {
        self.kernel.saturating_add(self.user)
    }

    /// Busy time: total - idle.
    #[must_use]
    pub fn busy(self) -> u64 {
        self.total().saturating_sub(self.idle)
    }
}

/// Physical memory from `GlobalMemoryStatusEx`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawPhysicalMemory {
    pub total_bytes: u64,
    pub available_bytes: u64,
}

/// Commit charge from `GetPerformanceInfo`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawCommit {
    pub commit_total_pages: u64,
    pub commit_limit_pages: u64,
    pub page_size_bytes: u64,
}

/// System identity fields collected via Windows APIs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawIdentity {
    pub hostname: String,
    pub os_version: String,
    pub architecture: String,
    pub logical_cores: u32,
    pub physical_memory_bytes: u64,
    /// Number of active processor groups. Single-group systems return 1.
    pub processor_group_count: u16,
}

/// Processor topology information for the host-size guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawProcessorTopology {
    pub active_logical_processors: u32,
    pub group_count: u16,
}

/// Owned logical-drive result from the Windows native source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawLogicalDrive {
    pub root: String,
    pub drive_type: u32,
    pub total_bytes: u64,
    pub free_bytes: u64,
}

/// Abstraction over native Windows system queries.
///
/// Production code calls FFI; tests inject a mock to exercise edge cases
/// without depending on the host state.
pub trait WindowsSource: Send + Sync + std::fmt::Debug {
    /// Enumerate ready logical drives and query their total/free capacity.
    fn logical_drives(&self) -> Result<Vec<RawLogicalDrive>, CollectError>;
    /// Read cumulative CPU time counters from `GetSystemTimes`.
    fn cpu_times(&self) -> Result<RawCpuTimes, CollectError>;

    /// Read the number of active logical processors across all groups.
    fn active_processor_count(&self) -> Result<u32, CollectError>;

    /// Read the processor topology (group count and total logical count).
    fn processor_topology(&self) -> Result<RawProcessorTopology, CollectError>;

    /// Read physical memory statistics from `GlobalMemoryStatusEx`.
    fn physical_memory(&self) -> Result<RawPhysicalMemory, CollectError>;

    /// Read commit charge from `GetPerformanceInfo`.
    fn commit(&self) -> Result<RawCommit, CollectError>;

    /// Read system identity fields.
    fn identity(&self) -> Result<RawIdentity, CollectError>;
}

/// Mock Windows source for unit tests. All fields are public so tests can
/// inject different values between successive calls.
#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)]
pub struct MockWindowsSource {
    pub drives: Vec<RawLogicalDrive>,
    pub cpu: RawCpuTimes,
    pub topology: RawProcessorTopology,
    pub memory: RawPhysicalMemory,
    pub commit: RawCommit,
    pub identity: RawIdentity,
    pub cpu_error: bool,
    pub topology_error: bool,
    pub memory_error: bool,
    pub commit_error: bool,
    pub identity_error: bool,
    pub drives_error: bool,
    /// When true, `cpu_times` increments `idle`, `kernel`, and `user` by a
    /// small delta on each call so successive samples produce a valid
    /// non-zero CPU interval.
    pub auto_increment_cpu: bool,
    pub(crate) cpu_call_count: std::sync::atomic::AtomicU32,
}

impl Clone for MockWindowsSource {
    fn clone(&self) -> Self {
        Self {
            drives: self.drives.clone(),
            cpu: self.cpu,
            topology: self.topology,
            memory: self.memory,
            commit: self.commit,
            identity: self.identity.clone(),
            cpu_error: self.cpu_error,
            topology_error: self.topology_error,
            memory_error: self.memory_error,
            commit_error: self.commit_error,
            identity_error: self.identity_error,
            drives_error: self.drives_error,
            auto_increment_cpu: self.auto_increment_cpu,
            cpu_call_count: std::sync::atomic::AtomicU32::new(
                self.cpu_call_count
                    .load(std::sync::atomic::Ordering::Relaxed),
            ),
        }
    }
}

impl MockWindowsSource {
    /// Build a mock returning sensible default values.
    #[must_use]
    pub fn success() -> Self {
        Self {
            drives: vec![RawLogicalDrive {
                root: "C:\\\\".to_string(),
                drive_type: DRIVE_FIXED,
                total_bytes: 100,
                free_bytes: 25,
            }],
            cpu: RawCpuTimes {
                idle: 8_000,
                kernel: 8_500,
                user: 1_000,
            },
            topology: RawProcessorTopology {
                active_logical_processors: 8,
                group_count: 1,
            },
            memory: RawPhysicalMemory {
                total_bytes: 16_000_000_000,
                available_bytes: 10_000_000_000,
            },
            commit: RawCommit {
                commit_total_pages: 200_000,
                commit_limit_pages: 800_000,
                page_size_bytes: 4096,
            },
            identity: RawIdentity {
                hostname: "win-server".to_string(),
                os_version: "10.0.22631".to_string(),
                architecture: "x86_64".to_string(),
                logical_cores: 8,
                physical_memory_bytes: 16_000_000_000,
                processor_group_count: 1,
            },
            cpu_error: false,
            topology_error: false,
            memory_error: false,
            commit_error: false,
            identity_error: false,
            drives_error: false,
            auto_increment_cpu: false,
            cpu_call_count: std::sync::atomic::AtomicU32::new(0),
        }
    }
}

impl WindowsSource for MockWindowsSource {
    fn logical_drives(&self) -> Result<Vec<RawLogicalDrive>, CollectError> {
        if self.drives_error {
            return Err(CollectError::new(
                crate::collector::error::CollectErrorKind::SourceUnavailable,
                "mock logical drive error",
            ));
        }
        Ok(self.drives.clone())
    }

    fn cpu_times(&self) -> Result<RawCpuTimes, CollectError> {
        if self.cpu_error {
            return Err(CollectError::new(
                crate::collector::error::CollectErrorKind::SourceUnavailable,
                "mock cpu error",
            ));
        }
        if self.auto_increment_cpu {
            use std::sync::atomic::Ordering;
            let call = self.cpu_call_count.fetch_add(1, Ordering::Relaxed);
            // Each call after the first adds ticks to produce a valid delta.
            let offset = u64::from(call) * 100;
            return Ok(RawCpuTimes {
                idle: self.cpu.idle + offset,
                kernel: self.cpu.kernel + offset,
                user: self.cpu.user + offset,
            });
        }
        Ok(self.cpu)
    }

    fn active_processor_count(&self) -> Result<u32, CollectError> {
        if self.topology_error {
            return Err(CollectError::new(
                crate::collector::error::CollectErrorKind::SourceUnavailable,
                "mock topology error",
            ));
        }
        Ok(self.topology.active_logical_processors)
    }

    fn processor_topology(&self) -> Result<RawProcessorTopology, CollectError> {
        if self.topology_error {
            return Err(CollectError::new(
                crate::collector::error::CollectErrorKind::SourceUnavailable,
                "mock topology error",
            ));
        }
        Ok(self.topology)
    }

    fn physical_memory(&self) -> Result<RawPhysicalMemory, CollectError> {
        if self.memory_error {
            return Err(CollectError::new(
                crate::collector::error::CollectErrorKind::SourceUnavailable,
                "mock memory error",
            ));
        }
        Ok(self.memory)
    }

    fn commit(&self) -> Result<RawCommit, CollectError> {
        if self.commit_error {
            return Err(CollectError::new(
                crate::collector::error::CollectErrorKind::SourceUnavailable,
                "mock commit error",
            ));
        }
        Ok(self.commit)
    }

    fn identity(&self) -> Result<RawIdentity, CollectError> {
        if self.identity_error {
            return Err(CollectError::new(
                crate::collector::error::CollectErrorKind::SourceUnavailable,
                "mock identity error",
            ));
        }
        Ok(self.identity.clone())
    }
}

// ---------------------------------------------------------------------------
// Production FFI implementation
// ---------------------------------------------------------------------------

/// Production implementation backed by Windows system APIs.
///
/// # Safety
///
/// All unsafe blocks in this implementation:
/// - Initialize structures with correct `cb`/`dwLength` sizes before API calls.
/// - Check return values and `GetLastError()` before reading output buffers.
/// - Copy owned data out of temporary buffers before returning.
/// - Never expose raw pointers or borrowed foreign memory across the boundary.
#[derive(Debug)]
pub struct NativeWindowsSource;

impl WindowsSource for NativeWindowsSource {
    fn logical_drives(&self) -> Result<Vec<RawLogicalDrive>, CollectError> {
        logical_drives()
    }

    fn cpu_times(&self) -> Result<RawCpuTimes, CollectError> {
        cpu_times()
    }

    fn active_processor_count(&self) -> Result<u32, CollectError> {
        active_processor_count()
    }

    fn processor_topology(&self) -> Result<RawProcessorTopology, CollectError> {
        processor_topology()
    }

    fn physical_memory(&self) -> Result<RawPhysicalMemory, CollectError> {
        physical_memory()
    }

    fn commit(&self) -> Result<RawCommit, CollectError> {
        commit()
    }

    fn identity(&self) -> Result<RawIdentity, CollectError> {
        collect_raw_identity()
    }
}

// ---------------------------------------------------------------------------
// Windows FFI helpers (native only)
//
// Raw extern declarations for Windows system APIs. This avoids depending
// on specific windows-sys feature flags and keeps the FFI surface minimal.
// All unsafe blocks document their safety invariants.
// ---------------------------------------------------------------------------

/// `ERROR_INSUFFICIENT_BUFFER` from Win32.
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;

/// `ALL_PROCESSOR_GROUPS` constant from Win32.
const ALL_PROCESSOR_GROUPS: u16 = 0x0000_FFFF;

/// `ComputerNameDnsHostname` from `COMPUTER_NAME_FORMAT` enum.
const COMPUTER_NAME_DNS_HOSTNAME: u32 = 3;
pub(crate) const DRIVE_FIXED: u32 = 3;
pub(crate) const DRIVE_REMOVABLE: u32 = 2;

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod ffi {
    //! Raw Windows FFI declarations for system information APIs.
    //!
    //! # Safety
    //!
    //! Every extern function is documented in Microsoft's Win32 API reference.
    //! Callers must ensure:
    //! - Structure `cbSize`/`dwLength` fields are initialized before the call.
    //! - Output buffers are valid for the declared length.
    //! - Return values are checked before reading output.

    /// `FILETIME` — 100-nanosecond intervals since January 1, 1601.
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct FileTime {
        pub dw_low_date_time: u32,
        pub dw_high_date_time: u32,
    }

    /// `MEMORYSTATUSEX` — extended memory status.
    #[repr(C)]
    pub struct MemoryStatusEx {
        pub dw_length: u32,
        pub memory_load: u32,
        pub ull_total_phys: u64,
        pub ull_avail_phys: u64,
        pub ull_total_page_file: u64,
        pub ull_avail_page_file: u64,
        pub ull_total_virtual: u64,
        pub ull_avail_virtual: u64,
        pub ull_avail_extended_virtual: u64,
    }

    /// `PERFORMANCE_INFORMATION` — system performance counters.
    #[repr(C)]
    pub struct PerformanceInformation {
        pub cb: usize,
        pub commit_total: usize,
        pub commit_limit: usize,
        pub commit_peak: usize,
        pub physical_available: usize,
        pub physical_total: usize,
        pub system_cache: usize,
        pub kernel_total: usize,
        pub kernel_paged: usize,
        pub kernel_nonpaged: usize,
        pub page_size: usize,
        pub handles_count: usize,
        pub process_count: usize,
        pub thread_count: usize,
    }

    /// `OSVERSIONINFOW` — OS version information.
    #[repr(C)]
    pub struct OsVersionInfoW {
        pub dw_os_version_info_size: u32,
        pub dw_major_version: u32,
        pub dw_minor_version: u32,
        pub dw_build_number: u32,
        pub dw_platform_id: u32,
        pub sz_c_version_string: [u16; 128],
    }

    /// `SYSTEM_INFO` — processor architecture info (simplified for `x86_64`).
    #[repr(C)]
    #[cfg(target_arch = "x86_64")]
    pub struct SystemInfo {
        pub w_processor_architecture: u16,
        pub w_reserved: u16,
        pub dw_page_size: u32,
        pub lp_min_application_address: *mut u8,
        pub lp_max_application_address: *mut u8,
        pub dw_active_processor_mask: usize,
        pub dw_number_of_processors: u32,
        pub dw_processor_type: u32,
        pub dw_allocation_granularity: u32,
        pub w_processor_level: u16,
        pub w_processor_revision: u16,
    }

    #[link(name = "kernel32")]
    #[link(name = "ntdll")]
    #[link(name = "psapi")]
    extern "system" {
        pub fn GetSystemTimes(
            idle_time: *mut FileTime,
            kernel_time: *mut FileTime,
            user_time: *mut FileTime,
        ) -> i32;

        pub fn GlobalMemoryStatusEx(lp_buffer: *mut MemoryStatusEx) -> i32;

        pub fn GetPerformanceInfo(
            p_performance_information: *mut PerformanceInformation,
            cb: u32,
        ) -> i32;

        pub fn GetActiveProcessorCount(group_number: u16) -> u32;

        pub fn GetActiveProcessorGroupCount() -> u16;

        pub fn GetComputerNameExW(name_type: u32, buffer: *mut u16, size: *mut u32) -> i32;

        pub fn RtlGetVersion(lp_version_information: *mut OsVersionInfoW) -> i32;

        pub fn GetSystemInfo(lp_system_info: *mut SystemInfo);

        pub fn GetLastError() -> u32;

        pub fn GetLogicalDriveStringsW(buffer_length: u32, buffer: *mut u16) -> u32;
        pub fn GetDriveTypeW(root_path_name: *const u16) -> u32;
        pub fn GetDiskFreeSpaceExW(
            directory_name: *const u16,
            free_bytes_available: *mut u64,
            total_number_of_bytes: *mut u64,
            total_number_of_free_bytes: *mut u64,
        ) -> i32;
    }
}

/// Convert a `FileTime` (100-nanosecond intervals since 1601) to a `u64`.
///
/// # Safety
///
/// Caller must ensure `ft` was initialized by a Windows API call.
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
unsafe fn filetime_to_u64(ft: ffi::FileTime) -> u64 {
    u64::from(ft.dw_low_date_time) | (u64::from(ft.dw_high_date_time) << 32)
}

fn logical_drives() -> Result<Vec<RawLogicalDrive>, CollectError> {
    #[cfg(target_os = "windows")]
    {
        let mut buffer = vec![0_u16; 256];
        let mut required = unsafe {
            // Safety: buffer is writable and its declared length matches its
            // allocation. The returned size is checked before parsing.
            ffi::GetLogicalDriveStringsW(buffer.len() as u32, buffer.as_mut_ptr())
        } as usize;
        if required >= buffer.len() {
            let size = required.checked_add(1).ok_or_else(|| {
                CollectError::new(
                    CollectErrorKind::Numeric,
                    "logical-drive buffer size overflow",
                )
            })?;
            buffer.resize(size, 0);
            required = unsafe {
                // Safety: the resized buffer is writable and the API receives
                // its exact capacity.
                ffi::GetLogicalDriveStringsW(buffer.len() as u32, buffer.as_mut_ptr())
            } as usize;
            if required >= buffer.len() {
                return Err(CollectError::new(
                    CollectErrorKind::SourceUnavailable,
                    "GetLogicalDriveStringsW buffer remained insufficient",
                ));
            }
        }

        let mut result = Vec::new();
        for root in buffer[..required].split(|value| *value == 0) {
            if root.is_empty() {
                continue;
            }
            let root = String::from_utf16(root).map_err(|_| {
                CollectError::new(
                    CollectErrorKind::Parse,
                    "logical drive root is invalid UTF-16",
                )
            })?;
            let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
            let drive_type = unsafe {
                // Safety: wide is NUL-terminated and lives through this call.
                ffi::GetDriveTypeW(wide.as_ptr())
            };
            if drive_type != DRIVE_FIXED && drive_type != DRIVE_REMOVABLE {
                continue;
            }
            let mut available = 0_u64;
            let mut total = 0_u64;
            let mut free = 0_u64;
            let success = unsafe {
                // Safety: all output pointers reference initialized writable
                // locals and the root pointer is valid for this call.
                ffi::GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, &mut total, &mut free)
            };
            if success == 0 || total == 0 || free > total {
                continue;
            }
            result.push(RawLogicalDrive {
                root,
                drive_type,
                total_bytes: total,
                free_bytes: free,
            });
        }
        result.sort_by(|left, right| left.root.cmp(&right.root));
        result.dedup_by(|left, right| left.root == right.root);
        Ok(result)
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(CollectError::new(
            CollectErrorKind::SourceUnavailable,
            "Windows drive APIs not available on this platform",
        ))
    }
}

/// Read cumulative CPU time counters from `GetSystemTimes`.
///
/// Returns 100-nanosecond interval counts for idle, kernel (including
/// idle), and user time.
fn cpu_times() -> Result<RawCpuTimes, CollectError> {
    #[cfg(target_os = "windows")]
    {
        use std::mem::MaybeUninit;

        unsafe {
            let mut idle = MaybeUninit::<ffi::FileTime>::uninit();
            let mut kernel = MaybeUninit::<ffi::FileTime>::uninit();
            let mut user = MaybeUninit::<ffi::FileTime>::uninit();

            let success =
                ffi::GetSystemTimes(idle.as_mut_ptr(), kernel.as_mut_ptr(), user.as_mut_ptr());

            if success == 0 {
                return Err(CollectError::new(
                    crate::collector::error::CollectErrorKind::SourceUnavailable,
                    "GetSystemTimes failed",
                ));
            }

            let idle_ft = idle.assume_init();
            let kernel_ft = kernel.assume_init();
            let user_ft = user.assume_init();

            Ok(RawCpuTimes {
                idle: filetime_to_u64(idle_ft),
                kernel: filetime_to_u64(kernel_ft),
                user: filetime_to_u64(user_ft),
            })
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(CollectError::new(
            crate::collector::error::CollectErrorKind::SourceUnavailable,
            "Windows CPU APIs not available on this platform",
        ))
    }
}

/// Read the total number of active logical processors across all groups.
fn active_processor_count() -> Result<u32, CollectError> {
    #[cfg(target_os = "windows")]
    {
        unsafe {
            let count = ffi::GetActiveProcessorCount(ALL_PROCESSOR_GROUPS);
            if count == 0 {
                return Err(CollectError::new(
                    crate::collector::error::CollectErrorKind::SourceUnavailable,
                    "GetActiveProcessorCount failed",
                ));
            }
            Ok(count)
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(CollectError::new(
            crate::collector::error::CollectErrorKind::SourceUnavailable,
            "Windows processor APIs not available on this platform",
        ))
    }
}

/// Read processor topology (group count and total logical processors).
fn processor_topology() -> Result<RawProcessorTopology, CollectError> {
    #[cfg(target_os = "windows")]
    {
        unsafe {
            let group_count = ffi::GetActiveProcessorGroupCount();
            let total = active_processor_count()?;
            Ok(RawProcessorTopology {
                active_logical_processors: total,
                group_count,
            })
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(CollectError::new(
            crate::collector::error::CollectErrorKind::SourceUnavailable,
            "Windows processor APIs not available on this platform",
        ))
    }
}

/// Read physical memory statistics from `GlobalMemoryStatusEx`.
fn physical_memory() -> Result<RawPhysicalMemory, CollectError> {
    #[cfg(target_os = "windows")]
    {
        use std::mem::MaybeUninit;

        unsafe {
            let mut mem_info = MaybeUninit::<ffi::MemoryStatusEx>::uninit();
            #[allow(clippy::cast_possible_truncation)]
            {
                (*mem_info.as_mut_ptr()).dw_length =
                    std::mem::size_of::<ffi::MemoryStatusEx>() as u32;
            }

            let success = ffi::GlobalMemoryStatusEx(mem_info.as_mut_ptr());

            if success == 0 {
                return Err(CollectError::new(
                    crate::collector::error::CollectErrorKind::SourceUnavailable,
                    "GlobalMemoryStatusEx failed",
                ));
            }

            let info = mem_info.assume_init();
            Ok(RawPhysicalMemory {
                total_bytes: info.ull_total_phys,
                available_bytes: info.ull_avail_phys,
            })
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(CollectError::new(
            crate::collector::error::CollectErrorKind::SourceUnavailable,
            "Windows memory APIs not available on this platform",
        ))
    }
}

/// Read commit charge from `GetPerformanceInfo`.
fn commit() -> Result<RawCommit, CollectError> {
    #[cfg(target_os = "windows")]
    {
        use std::mem::MaybeUninit;

        unsafe {
            let mut perf_info = MaybeUninit::<ffi::PerformanceInformation>::uninit();
            (*perf_info.as_mut_ptr()).cb = std::mem::size_of::<ffi::PerformanceInformation>();

            #[allow(clippy::cast_possible_truncation)]
            let cb = std::mem::size_of::<ffi::PerformanceInformation>() as u32;
            let success = ffi::GetPerformanceInfo(perf_info.as_mut_ptr(), cb);

            if success == 0 {
                let err = ffi::GetLastError();
                if err == ERROR_INSUFFICIENT_BUFFER {
                    return Err(CollectError::new(
                        crate::collector::error::CollectErrorKind::SourceUnavailable,
                        "GetPerformanceInfo requires larger buffer",
                    ));
                }
                return Err(CollectError::new(
                    crate::collector::error::CollectErrorKind::SourceUnavailable,
                    "GetPerformanceInfo failed",
                ));
            }

            let info = perf_info.assume_init();
            Ok(RawCommit {
                commit_total_pages: info.commit_total as u64,
                commit_limit_pages: info.commit_limit as u64,
                page_size_bytes: info.page_size as u64,
            })
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(CollectError::new(
            crate::collector::error::CollectErrorKind::SourceUnavailable,
            "Windows performance APIs not available on this platform",
        ))
    }
}

/// Collect raw identity fields via Windows APIs.
fn collect_raw_identity() -> Result<RawIdentity, CollectError> {
    let hostname = get_hostname()?;
    let (os_version, logical_cores) = get_os_info()?;
    let architecture = get_architecture();
    let physical_memory_bytes = physical_memory().map_or(0, |m| m.total_bytes);
    let topology = processor_topology().ok();

    Ok(RawIdentity {
        hostname,
        os_version,
        architecture,
        logical_cores,
        physical_memory_bytes,
        processor_group_count: topology.map_or(1, |t| t.group_count),
    })
}

/// Get the computer hostname via `GetComputerNameExW`.
fn get_hostname() -> Result<String, CollectError> {
    #[cfg(target_os = "windows")]
    {
        unsafe {
            let mut size: u32 = 0;
            // First call to determine required buffer size.
            ffi::GetComputerNameExW(COMPUTER_NAME_DNS_HOSTNAME, std::ptr::null_mut(), &mut size);

            if size == 0 {
                return Err(CollectError::new(
                    crate::collector::error::CollectErrorKind::SourceUnavailable,
                    "GetComputerNameExW returned zero size",
                ));
            }

            let mut buffer: Vec<u16> = vec![0; size as usize];
            let success =
                ffi::GetComputerNameExW(COMPUTER_NAME_DNS_HOSTNAME, buffer.as_mut_ptr(), &mut size);

            if success == 0 {
                return Err(CollectError::new(
                    crate::collector::error::CollectErrorKind::SourceUnavailable,
                    "GetComputerNameExW failed",
                ));
            }

            // Trim trailing NUL if present.
            if let Some(&0) = buffer.last() {
                buffer.pop();
            }

            String::from_utf16(&buffer).map_err(|_| {
                CollectError::new(
                    crate::collector::error::CollectErrorKind::Parse,
                    "hostname contains invalid UTF-16",
                )
            })
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(CollectError::new(
            crate::collector::error::CollectErrorKind::SourceUnavailable,
            "Windows hostname API not available on this platform",
        ))
    }
}

/// Get OS version and logical core count via `RtlGetVersion` and
/// `GetSystemInfo`.
#[allow(clippy::unnecessary_wraps)]
fn get_os_info() -> Result<(String, u32), CollectError> {
    #[cfg(target_os = "windows")]
    {
        use std::mem::MaybeUninit;

        unsafe {
            // RtlGetVersion doesn't require manifest version lying.
            let mut version_ex = MaybeUninit::<ffi::OsVersionInfoW>::uninit();
            #[allow(clippy::cast_possible_truncation)]
            {
                (*version_ex.as_mut_ptr()).dw_os_version_info_size =
                    std::mem::size_of::<ffi::OsVersionInfoW>() as u32;
            }

            let status = ffi::RtlGetVersion(version_ex.as_mut_ptr());

            let version = if status >= 0 {
                let v = version_ex.assume_init();
                format!(
                    "{}.{}.{}",
                    v.dw_major_version, v.dw_minor_version, v.dw_build_number
                )
            } else {
                "unknown".to_string()
            };

            // Get logical processor count from system info.
            #[cfg(target_arch = "x86_64")]
            {
                let mut sys_info = MaybeUninit::<ffi::SystemInfo>::uninit();
                ffi::GetSystemInfo(sys_info.as_mut_ptr());
                let info = sys_info.assume_init();
                let logical_cores = info.dw_number_of_processors;
                Ok((version, logical_cores))
            }
            #[cfg(not(target_arch = "x86_64"))]
            {
                // Fallback for non-x86_64: use the active processor count.
                let logical_cores = active_processor_count().unwrap_or(1);
                Ok((version, logical_cores))
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(CollectError::new(
            crate::collector::error::CollectErrorKind::SourceUnavailable,
            "Windows OS info APIs not available on this platform",
        ))
    }
}

/// Determine the target architecture string.
fn get_architecture() -> String {
    #[cfg(target_arch = "x86_64")]
    {
        "x86_64".to_string()
    }
    #[cfg(target_arch = "aarch64")]
    {
        "aarch64".to_string()
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        std::env::consts::ARCH.to_string()
    }
}
